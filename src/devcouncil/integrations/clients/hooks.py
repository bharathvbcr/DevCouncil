"""Hooks integration adapter."""
from __future__ import annotations

import json
import re
import shutil
from dataclasses import dataclass
from pathlib import Path

import typer

from devcouncil.executors.agent_registry import GEMINI_DEPRECATION_MESSAGE
from devcouncil.integrations.clients import common as _common
from devcouncil.integrations.clients import opencode as _opencode
_project_root = _common._project_root
_warn_if_verify_only = _common._warn_if_verify_only
_server_args = _common._server_args
_format_command = _common._format_command
_quote_powershell_arg = _common._quote_powershell_arg
_run = _common._run
_run_capture = _common._run_capture
_config_path = _common._config_path
_load_raw_config = _common._load_raw_config
_save_raw_config = _common._save_raw_config
_load_json = _common._load_json
_save_json = _common._save_json
_load_json_strict = _common._load_json_strict
_mutate_raw_config = _common._mutate_raw_config
_batched_raw_config = _common._batched_raw_config
_probe_mcp_tools = _common._probe_mcp_tools
_print_command = _common._print_command
_configure = _common._configure
resolve_dev_executable = _common.resolve_dev_executable
record_hook_dev_executable = _common.record_hook_dev_executable
check_hook_dev_executable = _common.check_hook_dev_executable
_apply_decouple_config_flags = _common._apply_decouple_config_flags

console = _common.console
OPENCODE_HOOK_PLUGIN_NAME = _common.OPENCODE_HOOK_PLUGIN_NAME
SUPPORTED_HOOK_TOOLS = _common.SUPPORTED_HOOK_TOOLS

# Claude Code SessionStart sources.  ``fork`` was added in v2.1.214 for
# ``--fork-session``, ``/fork`` and ``/branch``; before that a forked session
# reported ``resume``.  The value is only letters and ``|``, so Claude Code
# compares it as an exact alternation list rather than a regex — a missing
# entry silently never fires.
SESSION_START_MATCHER = "startup|resume|clear|compact|fork"

# Every hook event Claude Code recognizes, transcribed from the shipped 2.1.259
# binary's own event array (the one backing the `/hooks` browser and hook-config
# validation) and cross-checked against
# https://code.claude.com/docs/en/hooks#hook-lifecycle.  An event outside this set is
# not an error Claude Code raises: `claude plugin validate` reports
# "unknown hook event; entry ignored at runtime" and the entry is dropped, so without
# this list a typo installs a hook that can never fire while the writer reports
# success.  Kept whole rather than pruned to the events DevCouncil emits, so adding a
# handler is a one-line change here instead of a fresh research pass.
CLAUDE_HOOK_EVENTS: frozenset[str] = frozenset({
    "PreToolUse", "PostToolUse", "PostToolUseFailure", "PostToolBatch",
    "Notification", "UserPromptSubmit", "UserPromptExpansion",
    "SessionStart", "SessionEnd", "Stop", "StopFailure",
    "SubagentStart", "SubagentStop", "PreCompact", "PostCompact",
    "PreModelSwitch", "PostModelSwitch", "PermissionRequest", "PermissionDenied",
    "Setup", "TeammateIdle", "TaskCreated", "TaskCompleted",
    "Elicitation", "ElicitationResult", "ConfigChange",
    "WorktreeCreate", "WorktreeRemove", "InstructionsLoaded",
    "CwdChanged", "FileChanged", "DirectoryAdded", "MessageDisplay",
})

# FileChanged and StopFailure use a *narrower* exact-match character set than every
# other event (letters, digits, ``_`` and ``|`` only).  Anything else — a hyphen, a
# space, a dot — keeps the whole matcher on the regular-expression path, where for
# FileChanged it is additionally registered as a literal filename to watch.  Both
# failures are silent, so the set is checked rather than trusted.
# https://code.claude.com/docs/en/hooks#matcher-patterns
_NARROW_MATCHER_EVENTS = frozenset({"FileChanged", "StopFailure"})
_NARROW_MATCHER_RE = re.compile(r"^[A-Za-z0-9_|]+$")

GIT_MAP_HOOK_MARKER = "# DevCouncil: refresh repo map"
# Clients that install PreToolUse / BeforeTool / Cursor pre / OpenCode before containment.
CONTAINMENT_HOOK_CLIENTS = ("claude", "codex", "cursor", "grok", "opencode", "gemini")

_opencode_plugin_path = _opencode._opencode_plugin_path
_opencode_plugin_source = _opencode._opencode_plugin_source
_opencode_config_path = _opencode._opencode_config_path
_record_opencode_config = _opencode._record_opencode_config

# Claude Code's hook `timeout` is in SECONDS (command handlers default to 600).
# Named constants because there is a second generator for the plugin bundle
# (`integrations/claude_assets._plugin_hooks_json`) that missed the millisecond
# migration this module performs below and shipped 10000/150000 — a 2.8-hour and
# a 41.7-hour timeout — in the artifact other people install. One owner for the
# numbers is what stops that drifting again.
DEFAULT_HOOK_TIMEOUT_SECONDS = 10
STOP_GATE_HOOK_TIMEOUT_SECONDS = 150


def _stop_hook_timeout_seconds(project_root: Path) -> int:
    """Allow up to 150 seconds when the stop gate runs claims + verification."""
    try:
        from devcouncil.app.config import load_config

        sg = load_config(project_root).execution.stop_gate
        mode = (sg.mode or "off").strip().lower()
        if mode != "off" and (sg.check_claims or sg.verify_active_task):
            return STOP_GATE_HOOK_TIMEOUT_SECONDS
    except Exception:
        pass
    return DEFAULT_HOOK_TIMEOUT_SECONDS


@dataclass(frozen=True)
class ClaudeHookSpec:
    """One DevCouncil-owned Claude Code hook.

    Both writers of Claude hook config walk this table -- ``_install_claude_hooks``
    (``.claude/settings.local.json``) and the plugin bundle's ``hooks/hooks.json`` in
    ``integrations.claude_assets`` -- so the two cannot drift in events, matchers, or
    timeouts again. They differ only in how the command string is built (a resolved
    absolute ``dev`` path vs. ``${CLAUDE_PROJECT_DIR}``), which stays with each writer.
    """

    event: str
    """Claude Code hook event the handler is registered under."""
    matcher: str
    """Tool/source matcher; "" means every invocation of this event."""
    hook_event: str
    """Slug passed to ``devcouncil hook <slug>``."""
    name: str
    """DevCouncil-owned hook name; ``_upsert_hook`` replaces by it."""
    write_gate: bool = False
    """Blocking gate -- installed only under ``--write-gate``, removed otherwise."""
    stop_gate: bool = False
    """Runs the stop gate, so it needs the longer stop-gate timeout."""
    extra: tuple[str, ...] = ()
    """Extra argv after the shared hook command (e.g. ``--defer-batch``)."""
    status_message: str = ""
    """Spinner label while this hook runs; "" leaves Claude Code's generic one.

    Only the hooks that make a human wait carry one -- the stop gate can hold a turn
    for ``STOP_GATE_HOOK_TIMEOUT_SECONDS`` and the write gate sits in front of every
    Bash/Write/Edit, and an unexplained pause reads as the session having hung.
    """

    def timeout(self, project_root: Path) -> int:
        """Timeout in **seconds** -- Claude Code's ``timeout`` field is seconds, not ms."""
        if self.stop_gate:
            return _stop_hook_timeout_seconds(project_root)
        return DEFAULT_HOOK_TIMEOUT_SECONDS


CLAUDE_TOOL_MATCHER = "Bash|Write|Edit|MultiEdit"
# Refresh-only PostToolUse is always installed so assist mode keeps the map warm;
# ``--defer-batch`` queues paths so PostToolBatch can drain them once per batch.
# The blocking PreToolUse gate is the one write_gate=True entry. Lifecycle events
# after Stop cover status-on-start/prompt, teardown, compaction, subagent start and
# finish, and notifications. StopFailure covers the turn ends Stop never sees.
# FileChanged/CwdChanged/DirectoryAdded cover map freshness for changes that never
# pass through a tool call.
CLAUDE_HOOK_SPECS: tuple[ClaudeHookSpec, ...] = (
    ClaudeHookSpec(
        "PostToolUse",
        CLAUDE_TOOL_MATCHER,
        "post-tool-use",
        "devcouncil-post-tool-use",
        extra=("--defer-batch",),
    ),
    ClaudeHookSpec("PostToolBatch", "", "post-tool-batch", "devcouncil-post-tool-batch"),
    ClaudeHookSpec(
        "PreToolUse",
        CLAUDE_TOOL_MATCHER,
        "pre-tool-use",
        "devcouncil-pre-tool-use",
        write_gate=True,
        status_message="DevCouncil write gate",
    ),
    ClaudeHookSpec(
        "Stop",
        "",
        "agent-response",
        "devcouncil-agent-response-ready",
        stop_gate=True,
        status_message="DevCouncil stop gate (claims + verification)",
    ),
    # Fires *instead of* Stop when an API error ends the turn, so the stop gate above
    # never runs.  Fire-and-forget by contract (output and exit code are ignored), so
    # this can only record that nothing was checked -- which is the whole point.
    ClaudeHookSpec("StopFailure", "", "stop-failure", "devcouncil-stop-failure"),
    ClaudeHookSpec("SessionStart", SESSION_START_MATCHER, "session-start", "devcouncil-session-start"),
    ClaudeHookSpec("UserPromptSubmit", "", "user-prompt-submit", "devcouncil-user-prompt-submit"),
    ClaudeHookSpec("SessionEnd", "", "session-end", "devcouncil-session-end"),
    ClaudeHookSpec("PreCompact", "", "pre-compact", "devcouncil-pre-compact"),
    ClaudeHookSpec("PostCompact", "", "post-compact", "devcouncil-post-compact"),
    # SubagentStop gates a subagent's stop; without SubagentStart the subagent was
    # judged by a lease and task it was never told about.
    ClaudeHookSpec("SubagentStart", "", "subagent-start", "devcouncil-subagent-start"),
    ClaudeHookSpec(
        "SubagentStop",
        "",
        "subagent-stop",
        "devcouncil-subagent-stop",
        stop_gate=True,
        status_message="DevCouncil subagent stop gate",
    ),
    ClaudeHookSpec("Notification", "", "notification", "devcouncil-notification"),
    ClaudeHookSpec("FileChanged", "", "file-changed", "devcouncil-file-changed"),
    ClaudeHookSpec("CwdChanged", "", "cwd-changed", "devcouncil-cwd-changed"),
    ClaudeHookSpec("DirectoryAdded", "", "directory-added", "devcouncil-directory-added"),
)


def _hook_command_slugs() -> frozenset[str]:
    """Every ``devcouncil hook <slug>`` the CLI actually registers.

    Fails closed: a slug list we could not read must not read as "every slug is fine",
    which is how a hook that errors on every single fire gets installed and reported as
    a success.  If this import cannot resolve, the hooks it would have validated cannot
    run either, so refusing to write is the honest outcome.
    """
    try:
        import click
        from typer.main import get_command

        from devcouncil.cli.commands.hook import app as hook_app

        # `get_command` is typed as returning a bare `click.Command`; only a Group
        # carries subcommands. Narrowed rather than cast so a Typer app that
        # collapsed to a single command surfaces as the refusal below instead of an
        # AttributeError from inside the writer.
        command = get_command(hook_app)
        if not isinstance(command, click.Group):
            raise TypeError(f"`devcouncil hook` resolved to {type(command).__name__}, not a group")
        return frozenset(command.commands)
    except Exception as exc:  # noqa: BLE001 - reported, never swallowed
        raise ValueError(
            f"cannot enumerate `devcouncil hook` subcommands, so the hook config cannot "
            f"be checked against them; refusing to write it: {exc}"
        ) from exc


def validate_claude_hook_specs(
    specs: tuple[ClaudeHookSpec, ...], *, project_root: Path
) -> None:
    """Raise ``ValueError`` unless every spec is one Claude Code will actually run.

    Claude Code drops a hook it cannot make sense of *quietly* -- an unknown event is
    reported only by ``claude plugin validate`` ("unknown hook event; entry ignored at
    runtime"), a ``timeout`` that is not a positive number fails the entry's schema, and
    a command whose subcommand does not exist simply exits non-zero on every fire into a
    debug log nobody reads.  In all three cases the writer previously reported the same
    success it reports for a working install.  Both writers call this before touching a
    file, so a bad table fails the install rather than shipping dead hooks.
    """
    slugs = _hook_command_slugs()
    for spec in specs:
        where = f"{spec.name or '<unnamed>'} ({spec.event})"
        if spec.event not in CLAUDE_HOOK_EVENTS:
            raise ValueError(
                f"{where}: {spec.event!r} is not a Claude Code hook event; it would be "
                f"ignored at runtime. Known events: {', '.join(sorted(CLAUDE_HOOK_EVENTS))}"
            )
        if not spec.name.strip():
            raise ValueError(
                f"{spec.event}: hook has no DevCouncil name, so re-applying integration "
                "cannot replace or remove it"
            )
        if spec.hook_event not in slugs:
            raise ValueError(
                f"{where}: `devcouncil hook {spec.hook_event}` is not a registered "
                f"subcommand, so the hook would fail on every fire. "
                f"Known slugs: {', '.join(sorted(slugs))}"
            )
        if spec.event in _NARROW_MATCHER_EVENTS and spec.matcher:
            if not _NARROW_MATCHER_RE.match(spec.matcher):
                raise ValueError(
                    f"{where}: {spec.matcher!r} leaves this event's matcher on the "
                    "regular-expression path (only letters, digits, `_` and `|` are "
                    "exact-matched for FileChanged/StopFailure)"
                )
        timeout = spec.timeout(project_root)
        if not isinstance(timeout, int) or isinstance(timeout, bool) or timeout <= 0:
            raise ValueError(
                f"{where}: timeout must be a positive whole number of seconds, got "
                f"{timeout!r}; Claude Code rejects the entry otherwise"
            )


def claude_hook_specs(*, write_gate: bool, project_root: Path) -> tuple[ClaudeHookSpec, ...]:
    """The validated Claude hooks to emit for this posture; the blocking gate is opt-in.

    Validation covers the whole table, not just the returned subset: the write-gate
    entry is still written to ``.claude/settings.local.json`` under ``--write-gate``,
    and a broken spec should fail the install that would have shipped it.
    """
    validate_claude_hook_specs(CLAUDE_HOOK_SPECS, project_root=project_root)
    return tuple(spec for spec in CLAUDE_HOOK_SPECS if write_gate or not spec.write_gate)


def _hook_command(project_root: Path, client: str, event: str, *extra: str) -> str:
    # Absolute path to project-venv (or PATH) `dev` so a stale global install cannot
    # shadow the repo's CLI from PostToolUse / PreToolUse hooks.
    executable = resolve_dev_executable(project_root)
    return _format_command([
        executable,
        "hook",
        event,
        "--client",
        client,
        "--project-root",
        str(project_root),
        *extra,
    ])

def _upsert_hook(
    settings: dict,
    event: str,
    matcher: str,
    command: str,
    name: str,
    *,
    timeout: int = DEFAULT_HOOK_TIMEOUT_SECONDS,
    status_message: str = "",
) -> None:
    hooks = settings.setdefault("hooks", {})
    groups = hooks.setdefault(event, [])

    # A DevCouncil-owned hook name belongs to exactly one matcher group.  Groups are
    # keyed by matcher, so when the matcher value changes between releases the old
    # group is not the target group and the hook would stay registered under *both*
    # matchers — Claude Code then runs it twice for every event the two matchers
    # share.  Drop our name from every non-target group first, and prune a group
    # that held nothing else.  Hooks we do not own keep their group untouched.
    migrated: list = []
    for group in groups:
        if not isinstance(group, dict) or group.get("matcher") == matcher:
            migrated.append(group)
            continue
        inner = group.get("hooks")
        if not isinstance(inner, list):
            migrated.append(group)
            continue
        kept = [h for h in inner if not (isinstance(h, dict) and h.get("name") == name)]
        if len(kept) == len(inner):
            migrated.append(group)
        elif kept:
            migrated.append({**group, "hooks": kept})
    if migrated != groups:
        groups[:] = migrated

    target_group = None
    for group in groups:
        if group.get("matcher") == matcher:
            target_group = group
            break

    hook_payload: dict = {
        "type": "command",
        "name": name,
        "command": command,
        "timeout": timeout,
    }
    if status_message:
        # Optional spinner label (Claude Code hook-handler field `statusMessage`).
        # Omitted when empty rather than written as "" so a hook without one keeps
        # Claude Code's own default instead of a blank status.
        hook_payload["statusMessage"] = status_message

    if target_group is None:
        groups.append({"matcher": matcher, "hooks": [hook_payload]})
        return

    group_hooks = target_group.setdefault("hooks", [])
    replaced = False
    kept_hooks: list[dict] = []
    for hook in group_hooks:
        if hook.get("name") == name:
            if not replaced:
                # DevCouncil owns named hooks, including their timeout.  Re-applying
                # integration therefore migrates old millisecond values to the
                # seconds expected by Claude Code and Codex.
                kept_hooks.append(hook_payload)
                replaced = True
            continue
        kept_hooks.append(hook)
    if not replaced:
        kept_hooks.append(hook_payload)
    target_group["hooks"] = kept_hooks


def _remove_named_hook(settings: dict, event: str, name: str) -> None:
    """Remove a DevCouncil-owned hook and prune empty matcher groups/events."""
    hooks = settings.get("hooks")
    if not isinstance(hooks, dict):
        return
    groups = hooks.get(event)
    if not isinstance(groups, list):
        return
    kept_groups: list[dict] = []
    for group in groups:
        if not isinstance(group, dict):
            continue
        handlers = group.get("hooks")
        if isinstance(handlers, list):
            group = {
                **group,
                "hooks": [hook for hook in handlers if not isinstance(hook, dict) or hook.get("name") != name],
            }
        if group.get("hooks"):
            kept_groups.append(group)
    if kept_groups:
        hooks[event] = kept_groups
    else:
        hooks.pop(event, None)

def _ensure_codex_hooks_enabled(project_root: Path) -> Path:
    config_path = project_root / ".codex" / "config.toml"
    config_path.parent.mkdir(parents=True, exist_ok=True)
    existing = config_path.read_text(encoding="utf-8") if config_path.exists() else ""
    lines = existing.splitlines()
    updated: list[str] = []
    in_features = False
    saw_features = False
    inserted = False
    for line in lines:
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            if in_features and not inserted:
                updated.append("hooks = true")
                inserted = True
            in_features = stripped == "[features]"
            saw_features = saw_features or in_features
            updated.append(line)
            continue
        if in_features:
            key = stripped.split("=", 1)[0].strip() if "=" in stripped else ""
            if key in {"hooks", "codex_hooks"}:
                if not inserted:
                    updated.append("hooks = true")
                    inserted = True
                continue
        updated.append(line)
    if in_features and not inserted:
        updated.append("hooks = true")
        inserted = True
    if not saw_features:
        if updated and updated[-1].strip():
            updated.append("")
        updated.extend(["[features]", "hooks = true"])
    rendered = "\n".join(updated) + "\n"
    if rendered != existing:
        config_path.write_text(rendered, encoding="utf-8")
    return config_path

def _install_codex_hooks(project_root: Path, *, write_gate: bool = False) -> list[Path]:
    """Install Codex hooks. PostToolUse/lifecycle always; PreToolUse only with ``write_gate``.

    Assist (default) omits PreToolUse so interactive sessions are not re-bound to a
    leftover lease allowlist. Under ``write_gate`` (contain), PreToolUse is installed;
    Codex exit-2 deny on that path is intentional for autonomous runs.
    """
    path = project_root / ".codex" / "hooks.json"
    settings = _load_json(path)
    matcher = "Bash|shell_command|exec_command|local_shell|Write|Edit|MultiEdit|write_file|edit_file|apply_patch"
    _upsert_hook(
        settings,
        "PostToolUse",
        matcher,
        _hook_command(project_root, "codex", "post-tool-use"),
        "devcouncil-post-tool-use",
    )
    if write_gate:
        _upsert_hook(
            settings,
            "PreToolUse",
            matcher,
            _hook_command(project_root, "codex", "pre-tool-use"),
            "devcouncil-pre-tool-use",
        )
    else:
        _remove_named_hook(settings, "PreToolUse", "devcouncil-pre-tool-use")
    _upsert_hook(
        settings,
        "SessionStart",
        "startup|resume|clear|compact",
        _hook_command(project_root, "codex", "session-start"),
        "devcouncil-session-start",
    )
    _upsert_hook(
        settings,
        "Stop",
        "",
        _hook_command(project_root, "codex", "agent-response"),
        "devcouncil-agent-response-ready",
        timeout=_stop_hook_timeout_seconds(project_root),
    )
    _upsert_hook(
        settings,
        "SubagentStop",
        "",
        _hook_command(project_root, "codex", "subagent-stop"),
        "devcouncil-subagent-stop",
        timeout=_stop_hook_timeout_seconds(project_root),
    )
    _save_json(path, settings)

    def mutate(config: dict) -> None:
        codex = config.setdefault("integrations", {}).setdefault("codex", {})
        if isinstance(codex, dict):
            codex["write_gate"] = write_gate
        _common.seed_hook_gate_for_write_gate(config, write_gate=write_gate)

    _mutate_raw_config(project_root, mutate)
    return [path, _ensure_codex_hooks_enabled(project_root)]

def _install_gemini_hooks(project_root: Path, *, write_gate: bool = False) -> list[Path]:
    """Install Gemini hooks. AfterTool always; BeforeTool only with ``write_gate``."""
    path = project_root / ".gemini" / "settings.json"
    settings = _load_json(path)
    matcher = "run_shell_command|shell_command|write_file|edit_file|replace|apply_patch"
    _upsert_hook(
        settings,
        "AfterTool",
        matcher,
        _hook_command(project_root, "gemini", "post-tool-use"),
        "devcouncil-post-tool-use",
    )
    if write_gate:
        _upsert_hook(
            settings,
            "BeforeTool",
            matcher,
            _hook_command(project_root, "gemini", "pre-tool-use"),
            "devcouncil-pre-tool-use",
        )
    else:
        _remove_named_hook(settings, "BeforeTool", "devcouncil-pre-tool-use")
    _save_json(path, settings)

    def mutate(config: dict) -> None:
        gemini = config.setdefault("integrations", {}).setdefault("gemini", {})
        if isinstance(gemini, dict):
            gemini["write_gate"] = write_gate
        _common.seed_hook_gate_for_write_gate(config, write_gate=write_gate)

    _mutate_raw_config(project_root, mutate)
    return [path]

def _upsert_cursor_hook(settings: dict, event: str, matcher: str, command: str) -> None:
    hooks = settings.setdefault("hooks", {})
    entries = hooks.setdefault(event, [])
    for entry in entries:
        if entry.get("command") == command:
            # Refresh matcher on re-apply (e.g. drop Read|Task).
            if matcher:
                entry["matcher"] = matcher
            elif "matcher" in entry:
                entry.pop("matcher", None)
            return
    payload: dict = {"command": command}
    if matcher:
        payload["matcher"] = matcher
    entries.append(payload)


def _remove_cursor_hook(settings: dict, event: str, *, command_substr: str) -> None:
    """Remove Cursor hook entries whose command contains ``command_substr``."""
    hooks = settings.get("hooks")
    if not isinstance(hooks, dict):
        return
    entries = hooks.get(event)
    if not isinstance(entries, list):
        return
    kept = [
        entry
        for entry in entries
        if not (isinstance(entry, dict) and command_substr in str(entry.get("command") or ""))
    ]
    if kept:
        hooks[event] = kept
    else:
        hooks.pop(event, None)


def _install_cursor_hooks(project_root: Path, *, write_gate: bool = False) -> list[Path]:
    """Install Cursor hooks. PostToolUse always; PreToolUse only with ``write_gate``."""
    root = project_root.expanduser().resolve()
    path = root / ".cursor" / "hooks.json"
    settings = _load_json(path)
    settings.setdefault("version", 1)
    matcher = "Shell|Write|Edit|MultiEdit"
    post_cmd = _hook_command(root, "cursor", "post-tool-use")
    pre_cmd = _hook_command(root, "cursor", "pre-tool-use")
    # Drop prior DevCouncil entries (absolute/relative path drift) before upsert.
    _remove_cursor_hook(settings, "postToolUse", command_substr="post-tool-use")
    _remove_cursor_hook(settings, "preToolUse", command_substr="pre-tool-use")
    _upsert_cursor_hook(settings, "postToolUse", matcher, post_cmd)
    if write_gate:
        _upsert_cursor_hook(settings, "preToolUse", matcher, pre_cmd)
    _save_json(path, settings)

    def mutate(config: dict) -> None:
        cursor = config.setdefault("integrations", {}).setdefault("cursor", {})
        cursor.update({
            "hooks_path": str(path.relative_to(root)),
            "write_gate": write_gate,
        })
        _common.seed_hook_gate_for_write_gate(config, write_gate=write_gate)

    _mutate_raw_config(root, mutate)
    return [path]


def _devmap_hook_command(project_root: Path, *args: str) -> str:
    """Absolute-enough command string for DevMap session hooks."""
    executable = shutil.which("devmap") or "devmap"
    db = project_root / ".devcouncil" / "codeintel" / "devmap.sqlite"
    return _format_command([executable, "--db", str(db), *args])


def _install_devmap_session_hooks(settings: dict, project_root: Path) -> None:
    """SessionStart/SessionEnd insights for hosts that share Claude's hook schema."""
    _upsert_hook(
        settings,
        "SessionStart",
        "startup|resume|clear|compact|fork",
        _devmap_hook_command(project_root, "session-report", "--last"),
        "devmap-session-start",
        timeout=5,
        status_message="Dev Map last session",
    )
    _upsert_hook(
        settings,
        "SessionEnd",
        "",
        _devmap_hook_command(project_root, "session-report"),
        "devmap-session-end",
        timeout=10,
        status_message="Dev Map session report",
    )


def _install_grok_hooks(project_root: Path, *, write_gate: bool = False) -> list[Path]:
    """Install Grok hooks. PostToolUse always; PreToolUse only with ``write_gate``."""
    hooks_dir = project_root / ".grok" / "hooks"
    hooks_dir.mkdir(parents=True, exist_ok=True)
    path = hooks_dir / "devcouncil.json"
    settings = _load_json(path)
    matcher = "Bash|Write|Edit|MultiEdit|run_terminal_cmd|write_file|edit_file|apply_patch"
    _upsert_hook(
        settings,
        "PostToolUse",
        matcher,
        _hook_command(project_root, "grok", "post-tool-use"),
        "devcouncil-post-tool-use",
    )
    _install_devmap_session_hooks(settings, project_root)
    if write_gate:
        _upsert_hook(
            settings,
            "PreToolUse",
            matcher,
            _hook_command(project_root, "grok", "pre-tool-use"),
            "devcouncil-pre-tool-use",
        )
    else:
        _remove_named_hook(settings, "PreToolUse", "devcouncil-pre-tool-use")
    _save_json(path, settings)

    def mutate(config: dict) -> None:
        grok = config.setdefault("integrations", {}).setdefault("grok", {})
        grok.update({
            "hooks_path": str(path.relative_to(project_root)),
            "write_gate": write_gate,
        })
        _common.seed_hook_gate_for_write_gate(config, write_gate=write_gate)

    _mutate_raw_config(project_root, mutate)
    return [path]


def _opencode_plugin_body(*, write_gate: bool) -> str:
    """Generate OpenCode plugin source. Pre-tool gate only when write_gate."""
    lines = [
        'import { spawnSync } from "node:child_process";',
        "",
        "const projectRoot = process.env.DEVCOUNCIL_PROJECT_ROOT || process.cwd();",
        "",
        "function runHook(event, payload) {",
        '  const args = ["hook", event, "--client", "opencode", "--project-root", projectRoot];',
        "  const result = spawnSync(\"devcouncil\", args, {",
        "    input: JSON.stringify(payload ?? {}),",
        '    encoding: "utf-8",',
        "    env: { ...process.env, DEVCOUNCIL_PROJECT_ROOT: projectRoot },",
        "  });",
        "  if (result.status === 2) {",
        '    throw new Error(result.stderr || result.stdout || "DevCouncil blocked the tool call.");',
        "  }",
        "}",
        "",
        "export const DevCouncilOpenCodeHook = async () => ({",
    ]
    if write_gate:
        lines.extend(
            [
                '  "tool.execute.before": async (input, output) => {',
                "    runHook(\"pre-tool-use\", { tool: input.tool, arguments: output.args });",
                "  },",
            ]
        )
    lines.extend(
        [
            '  "tool.execute.after": async (input, output) => {',
            "    runHook(\"post-tool-use\", { tool: input.tool, arguments: output.args });",
            "  },",
            "});",
            "",
        ]
    )
    return "\n".join(lines)


def _install_opencode_hooks(project_root: Path, *, write_gate: bool = False) -> list[Path]:
    destination = _opencode_plugin_path(project_root)
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(_opencode_plugin_body(write_gate=write_gate), encoding="utf-8")

    path = _opencode_config_path(project_root)
    data = _load_json_strict(path, "OpenCode") if path.exists() else {"$schema": "https://opencode.ai/config.json"}
    data.setdefault("$schema", "https://opencode.ai/config.json")
    plugins_raw = data.setdefault("plugin", [])
    if not isinstance(plugins_raw, list):
        plugins_raw = []
        data["plugin"] = plugins_raw
    plugins: list[str] = [str(item) for item in plugins_raw]
    data["plugin"] = plugins
    plugin_ref = f"./.devcouncil/integrations/{OPENCODE_HOOK_PLUGIN_NAME}"
    if plugin_ref not in plugins:
        plugins.append(plugin_ref)
    _save_json(path, data)

    def mutate(config: dict) -> None:
        opencode = config.setdefault("integrations", {}).setdefault("opencode", {})
        opencode.update({"write_gate": write_gate})
        _common.seed_hook_gate_for_write_gate(config, write_gate=write_gate)

    _mutate_raw_config(project_root, mutate)
    _record_opencode_config(project_root)
    return [destination, path]

def _install_claude_hooks(project_root: Path, *, write_gate: bool = False) -> list[Path]:
    """Install DevCouncil's Claude Code hooks into .claude/settings.local.json.

    By default this installs *assistive* lifecycle hooks plus a refresh-only
    **PostToolUse** hook (map auto-refresh; never gates writes). These never block
    a tool call.

    The blocking pre-action **write-gate** (**PreToolUse**, which denies any
    Bash/Write/Edit not authorized by an active task lease) is installed ONLY when
    ``write_gate`` is True. It is meant for autonomous executor runs, not interactive
    human sessions — in an interactive session there is no task lease, so the gate would
    fail-closed and deny every command. (``dev run --executor claude`` does its own
    post-hoc scope enforcement and does not depend on this hook, so leaving PreToolUse
    off by default loses no containment.)

    Refuses to write a table Claude Code would silently ignore -- see
    :func:`validate_claude_hook_specs`. The check runs before the file is opened so a
    bad spec leaves the previous, working settings in place rather than half-replacing
    them."""
    validate_claude_hook_specs(CLAUDE_HOOK_SPECS, project_root=project_root)
    path = project_root / ".claude" / "settings.local.json"
    settings = _load_json(path)
    for spec in CLAUDE_HOOK_SPECS:
        if spec.write_gate and not write_gate:
            # Reapplying the default assist integration must actually disable a
            # previously opted-in blocking gate; otherwise interactive sessions stay
            # fail-closed forever despite --no-write-gate.
            _remove_named_hook(settings, spec.event, spec.name)
            continue
        _upsert_hook(
            settings,
            spec.event,
            spec.matcher,
            _hook_command(project_root, "claude", spec.hook_event, *spec.extra),
            spec.name,
            timeout=spec.timeout(project_root),
            status_message=spec.status_message,
        )
    _save_json(path, settings)
    # Keep runtime gate posture and assist/contain flag in sync with --write-gate.
    def mutate(config: dict) -> None:
        claude = config.setdefault("integrations", {}).setdefault("claude", {})
        if isinstance(claude, dict):
            claude["write_gate"] = write_gate
        _common.seed_hook_gate_for_write_gate(config, write_gate=write_gate)

    _mutate_raw_config(project_root, mutate)
    return [path]

def _preview_hook_paths(project_root: Path, tool: str) -> list[tuple[str, Path]]:
    paths = {
        "codex": [project_root / ".codex" / "hooks.json", project_root / ".codex" / "config.toml"],
        "gemini": [project_root / ".gemini" / "settings.json"],
        "claude": [project_root / ".claude" / "settings.local.json"],
        "cursor": [project_root / ".cursor" / "hooks.json"],
        "grok": [project_root / ".grok" / "hooks" / "devcouncil.json"],
        "opencode": [_opencode_plugin_path(project_root), _opencode_config_path(project_root)],
    }
    selected: tuple[str, ...]
    if tool == "all":
        selected = (*SUPPORTED_HOOK_TOOLS, "opencode")
    elif tool == "opencode":
        selected = ("opencode",)
    else:
        selected = (tool,)
    return [(client, path) for client in selected for path in paths.get(client, [])]

def _configure_native_hooks(
    project_root: Path, tool: str = "all", apply: bool = False, *, write_gate: bool = False, claude_write_gate: bool | None = None
) -> None:
    allowed = {"all", *SUPPORTED_HOOK_TOOLS, "gemini", "opencode"}
    if tool not in allowed:
        console.print("[red]--tool must be one of: all, codex, gemini, claude, cursor, grok, opencode.[/red]")
        raise typer.Exit(code=2)

    if tool == "gemini":
        console.print(f"[yellow]{GEMINI_DEPRECATION_MESSAGE}[/yellow]")

    if not apply:
        console.print("[bold]Native hook config preview[/bold]")
        for client, path in _preview_hook_paths(project_root, tool):
            console.print(f"{client}: {path}", soft_wrap=True)
        console.print("[yellow]Preview only. Rerun with --apply to write hook config files.[/yellow]")
        return

    selected: tuple[str, ...]
    if tool == "all":
        selected = (*SUPPORTED_HOOK_TOOLS, "opencode")
    elif tool == "opencode":
        selected = ("opencode",)
    else:
        selected = (tool,)
    # Prefer write_gate; accept legacy claude_write_gate kw from older callers.
    if claude_write_gate is not None:
        write_gate = bool(claude_write_gate)

    installers = {
        # Blocking PreToolUse/BeforeTool write-gate is opt-in for all clients.
        "codex": lambda root: _install_codex_hooks(root, write_gate=write_gate),
        "gemini": lambda root: _install_gemini_hooks(root, write_gate=write_gate),
        "claude": lambda root: _install_claude_hooks(root, write_gate=write_gate),
        "cursor": lambda root: _install_cursor_hooks(root, write_gate=write_gate),
        "grok": lambda root: _install_grok_hooks(root, write_gate=write_gate),
        "opencode": lambda root: _install_opencode_hooks(root, write_gate=write_gate),
    }
    # Batch the per-installer config.yaml record updates (cursor/opencode)
    # into one load/save instead of re-parsing YAML per tool.
    with _batched_raw_config(project_root):
        for client in selected:
            try:
                written = installers[client](project_root)
            except (ValueError, FileNotFoundError) as exc:
                console.print(f"[red]{client} hook setup failed: {exc}[/red]")
                raise typer.Exit(code=1) from exc
            console.print(f"[green]{client} native hooks configured:[/green] {', '.join(str(path) for path in written)}")
        if any(client in _common.STOP_HOOK_TOOLS for client in selected):
            _mutate_raw_config(project_root, _common.seed_stop_gate_assist_if_unset)
    record_hook_dev_executable(project_root)


def _strip_devcouncil_hooks_from_settings(settings: dict) -> bool:
    """Remove every hook entry whose command invokes ``devcouncil hook`` / ``dev hook``.

    Returns True when settings were mutated. Prunes empty matcher groups and events.
    """
    hooks = settings.get("hooks")
    if not isinstance(hooks, dict):
        return False
    changed = False
    for event in list(hooks):
        groups = hooks.get(event)
        if not isinstance(groups, list):
            continue
        # Claude/Codex/Gemini/Grok: list of matcher groups with nested hooks.
        if groups and isinstance(groups[0], dict) and "hooks" in groups[0]:
            kept_groups: list[dict] = []
            for group in groups:
                if not isinstance(group, dict):
                    kept_groups.append(group)
                    continue
                inner = group.get("hooks", [])
                if not isinstance(inner, list):
                    kept_groups.append(group)
                    continue
                inner_kept = [
                    h for h in inner
                    if not (
                        isinstance(h, dict)
                        and (
                            "devcouncil hook" in str(h.get("command", ""))
                            or str(h.get("name", "")).startswith("devcouncil-")
                            or re.search(r"(^|[\s/\\])dev(\.exe)?\s+hook\b", str(h.get("command", "")))
                        )
                    )
                ]
                if len(inner_kept) != len(inner):
                    changed = True
                if inner_kept:
                    group = {**group, "hooks": inner_kept}
                    kept_groups.append(group)
            if kept_groups:
                if hooks.get(event) != kept_groups:
                    hooks[event] = kept_groups
                    changed = True
            else:
                hooks.pop(event, None)
                changed = True
            continue
        # Cursor-style: flat list of {command, matcher?} entries.
        kept_entries = [
            entry
            for entry in groups
            if not (
                isinstance(entry, dict)
                and (
                    "devcouncil hook" in str(entry.get("command", ""))
                    or "pre-tool-use" in str(entry.get("command", ""))
                    or "post-tool-use" in str(entry.get("command", ""))
                    or re.search(r"(^|[\s/\\])dev(\.exe)?\s+hook\b", str(entry.get("command", "")))
                )
            )
        ]
        if len(kept_entries) != len(groups):
            changed = True
            if kept_entries:
                hooks[event] = kept_entries
            else:
                hooks.pop(event, None)
    if not hooks:
        settings.pop("hooks", None)
        changed = True
    return changed


def _strip_opencode_before_handler_text(text: str) -> tuple[str, bool]:
    """Surgically drop ``tool.execute.before`` from an OpenCode plugin body.

    Returns (new_text, stripped). Falls back to a full assist-mode regen when the
    before-handler shape cannot be parsed safely.
    """
    marker = '"tool.execute.before"'
    if marker not in text:
        return text, False
    known_contain = _opencode_plugin_body(write_gate=True)
    known_assist = _opencode_plugin_body(write_gate=False)
    if text == known_contain:
        return known_assist, True
    idx = text.find(marker)
    # Walk back to the start of the property line.
    line_start = text.rfind("\n", 0, idx) + 1
    # Find the opening brace of the async handler body after `=>`.
    arrow = text.find("=>", idx)
    if arrow < 0:
        return known_assist, True
    brace = text.find("{", arrow)
    if brace < 0:
        return known_assist, True
    depth = 0
    end = brace
    for i in range(brace, len(text)):
        ch = text[i]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                end = i + 1
                break
    else:
        return known_assist, True
    # Consume trailing comma + whitespace/newlines after the handler.
    while end < len(text) and text[end] in " \t":
        end += 1
    if end < len(text) and text[end] == ",":
        end += 1
    while end < len(text) and text[end] in " \t\r":
        end += 1
    if end < len(text) and text[end] == "\n":
        end += 1
    stripped = text[:line_start] + text[end:]
    if marker in stripped:
        return known_assist, True
    return stripped, True


def _strip_command_hooks_from_event(
    settings: dict,
    event: str,
    *,
    command_substr: str,
) -> None:
    """Drop nested hook handlers whose command contains ``command_substr``; prune empties."""
    hooks = settings.get("hooks")
    if not isinstance(hooks, dict):
        return
    groups = hooks.get(event)
    if not isinstance(groups, list):
        return
    kept_groups: list[dict] = []
    for group in groups:
        if not isinstance(group, dict):
            kept_groups.append(group)
            continue
        inner = group.get("hooks", [])
        if not isinstance(inner, list):
            kept_groups.append(group)
            continue
        inner_kept = [
            h
            for h in inner
            if not (isinstance(h, dict) and command_substr in str(h.get("command", "")))
        ]
        if inner_kept:
            kept_groups.append({**group, "hooks": inner_kept})
    if kept_groups:
        hooks[event] = kept_groups
    else:
        hooks.pop(event, None)
    if not hooks:
        settings.pop("hooks", None)


def _strip_claude_containment(project_root: Path) -> list[str]:
    """Remove PreToolUse containment only; leave PostToolUse/lifecycle hooks intact."""
    path = project_root / ".claude" / "settings.local.json"
    if not path.exists():
        return []
    settings = _load_json(path)
    before = json.dumps(settings, sort_keys=True)
    _remove_named_hook(settings, "PreToolUse", "devcouncil-pre-tool-use")
    # Also drop unnamed PreToolUse entries that still invoke the pre-tool-use hook.
    _strip_command_hooks_from_event(settings, "PreToolUse", command_substr="pre-tool-use")
    if json.dumps(settings, sort_keys=True) == before:
        return []
    if settings:
        _save_json(path, settings)
    elif path.exists():
        path.unlink()
    return [f"stripped PreToolUse from {path.name}"]


def _strip_codex_containment(project_root: Path) -> list[str]:
    path = project_root / ".codex" / "hooks.json"
    if not path.exists():
        return []
    settings = _load_json(path)
    before = json.dumps(settings, sort_keys=True)
    _remove_named_hook(settings, "PreToolUse", "devcouncil-pre-tool-use")
    _strip_command_hooks_from_event(settings, "PreToolUse", command_substr="pre-tool-use")
    if json.dumps(settings, sort_keys=True) == before:
        return []
    _save_json(path, settings)
    return [f"stripped PreToolUse from {path.relative_to(project_root)}"]


def _strip_gemini_containment(project_root: Path) -> list[str]:
    path = project_root / ".gemini" / "settings.json"
    if not path.exists():
        return []
    settings = _load_json(path)
    before = json.dumps(settings, sort_keys=True)
    _remove_named_hook(settings, "BeforeTool", "devcouncil-pre-tool-use")
    _strip_command_hooks_from_event(settings, "BeforeTool", command_substr="pre-tool-use")
    if json.dumps(settings, sort_keys=True) == before:
        return []
    _save_json(path, settings)
    return [f"stripped BeforeTool from {path.relative_to(project_root)}"]


def _strip_cursor_containment(project_root: Path) -> list[str]:
    path = project_root / ".cursor" / "hooks.json"
    if not path.exists():
        return []
    settings = _load_json(path)
    before = json.dumps(settings, sort_keys=True)
    _remove_cursor_hook(settings, "preToolUse", command_substr="pre-tool-use")
    if json.dumps(settings, sort_keys=True) == before:
        return []
    hooks = settings.get("hooks")
    if isinstance(hooks, dict) and not hooks:
        settings.pop("hooks", None)
    if settings.get("hooks") or settings.get("version") is not None:
        _save_json(path, settings)
    elif path.exists():
        path.unlink()
    return [f"stripped preToolUse from {path.relative_to(project_root)}"]


def _strip_grok_containment(project_root: Path) -> list[str]:
    path = project_root / ".grok" / "hooks" / "devcouncil.json"
    if not path.exists():
        return []
    settings = _load_json(path)
    before = json.dumps(settings, sort_keys=True)
    _remove_named_hook(settings, "PreToolUse", "devcouncil-pre-tool-use")
    _strip_command_hooks_from_event(settings, "PreToolUse", command_substr="pre-tool-use")
    if json.dumps(settings, sort_keys=True) == before:
        return []
    _save_json(path, settings)
    return [f"stripped PreToolUse from {path.relative_to(project_root)}"]


def _strip_opencode_containment(project_root: Path) -> list[str]:
    path = _opencode_plugin_path(project_root)
    if not path.exists():
        return []
    text = path.read_text(encoding="utf-8")
    new_text, stripped = _strip_opencode_before_handler_text(text)
    if not stripped or new_text == text:
        return []
    path.write_text(new_text, encoding="utf-8")
    return [f"stripped tool.execute.before from {path.relative_to(project_root)}"]


_CONTAINMENT_STRIPPERS = {
    "claude": _strip_claude_containment,
    "codex": _strip_codex_containment,
    "gemini": _strip_gemini_containment,
    "cursor": _strip_cursor_containment,
    "grok": _strip_grok_containment,
    "opencode": _strip_opencode_containment,
}


def _decouple_client_hooks(project_root: Path, client: str) -> list[str]:
    """Strip containment hooks for one client and force assist config flags."""
    stripper = _CONTAINMENT_STRIPPERS.get(client)
    changes: list[str] = []
    if stripper is not None:
        changes.extend(stripper(project_root))
    changes.extend(_apply_decouple_config_flags(project_root, client if client in _common.CLIENTS_WITH_WRITE_GATE else None))
    return changes


def _decouple_all_containment_hooks(project_root: Path) -> list[str]:
    """Strip PreToolUse/before containment across all hook clients (no reinstall)."""
    changes: list[str] = []
    with _batched_raw_config(project_root):
        for client in CONTAINMENT_HOOK_CLIENTS:
            changes.extend(_CONTAINMENT_STRIPPERS[client](project_root))

        def mutate(config: dict) -> None:
            for client in _common.CLIENTS_WITH_WRITE_GATE:
                entry = config.setdefault("integrations", {}).setdefault(client, {})
                if isinstance(entry, dict) and entry.get("write_gate") is not False:
                    entry["write_gate"] = False
                    changes.append(f"integrations.{client}.write_gate=false")
            execution = config.setdefault("execution", {})
            hook_gate = execution.setdefault("hook_gate", {})
            if not isinstance(hook_gate, dict):
                hook_gate = {}
                execution["hook_gate"] = hook_gate
            if hook_gate.get("mode") != "off":
                hook_gate["mode"] = "off"
                changes.append("execution.hook_gate.mode=off")
            stop_gate = execution.setdefault("stop_gate", {})
            if not isinstance(stop_gate, dict):
                stop_gate = {}
                execution["stop_gate"] = stop_gate
            mode = str(stop_gate.get("mode") or "").strip().lower()
            if mode == "block":
                stop_gate["mode"] = "assist"
                changes.append("execution.stop_gate.mode=assist")

        _mutate_raw_config(project_root, mutate)
    seen: set[str] = set()
    unique: list[str] = []
    for item in changes:
        if item not in seen:
            seen.add(item)
            unique.append(item)
    return unique


def _uninstall_named_hook_file(path: Path, project_root: Path, *, delete_if_empty: bool = True) -> list[str]:
    """Strip all DevCouncil hooks from a Claude/Codex-style hooks JSON file."""
    if not path.exists():
        return []
    settings = _load_json(path)
    before = json.dumps(settings, sort_keys=True)
    _strip_devcouncil_hooks_from_settings(settings)
    if json.dumps(settings, sort_keys=True) == before:
        return []
    rel = str(path.relative_to(project_root)) if path.is_relative_to(project_root) else str(path)
    if not settings and delete_if_empty:
        path.unlink()
        return [f"deleted empty {rel}"]
    # Cursor: delete when only version remains and hooks gone.
    if delete_if_empty and set(settings.keys()) <= {"version"} and "hooks" not in settings:
        path.unlink()
        return [f"deleted empty {rel}"]
    _save_json(path, settings)
    return [f"stripped DevCouncil hooks from {rel}"]


def _uninstall_claude_hooks(project_root: Path) -> list[str]:
    return _uninstall_named_hook_file(project_root / ".claude" / "settings.local.json", project_root, delete_if_empty=False)


def _uninstall_codex_hooks(project_root: Path) -> list[str]:
    # Do not touch [features] hooks in config.toml — only strip hooks.json entries.
    return _uninstall_named_hook_file(project_root / ".codex" / "hooks.json", project_root)


def _uninstall_gemini_hooks(project_root: Path) -> list[str]:
    return _uninstall_named_hook_file(project_root / ".gemini" / "settings.json", project_root, delete_if_empty=False)


def _uninstall_cursor_hooks(project_root: Path) -> list[str]:
    return _uninstall_named_hook_file(project_root / ".cursor" / "hooks.json", project_root)


def _uninstall_grok_hooks(project_root: Path) -> list[str]:
    path = project_root / ".grok" / "hooks" / "devcouncil.json"
    if not path.exists():
        return []
    path.unlink()
    return [str(path.relative_to(project_root))]


def _uninstall_opencode_hooks(project_root: Path) -> list[str]:
    """Remove the generated OpenCode hook plugin file and its plugin[] registration.

    Leaves ``mcp.devcouncil`` alone — that is full client uninstall territory.
    """
    removed: list[str] = []
    root = project_root.expanduser().resolve()
    plugin = _opencode_plugin_path(root)
    if plugin.exists():
        plugin.unlink()
        removed.append(str(plugin.relative_to(root)))
    path = _opencode_config_path(root)
    if path.exists():
        data = _load_json(path)
        plugins = data.get("plugin")
        changed = False
        if isinstance(plugins, list):
            kept = [p for p in plugins if not _opencode._is_opencode_plugin_ref(p)]
            if len(kept) != len(plugins):
                if kept:
                    data["plugin"] = kept
                else:
                    data.pop("plugin", None)
                changed = True
                removed.append(f"plugin entry {_opencode._opencode_plugin_ref_canonical()}")
        elif isinstance(plugins, str) and _opencode._is_opencode_plugin_ref(plugins):
            data.pop("plugin", None)
            changed = True
            removed.append(f"plugin entry {_opencode._opencode_plugin_ref_canonical()}")
        if changed:
            if data:
                _save_json(path, data)
            elif path.exists():
                path.unlink()
    return removed


def _strip_git_map_hook_block(text: str, marker: str = GIT_MAP_HOOK_MARKER) -> str:
    """Remove DevCouncil map-refresh marker + following command line from a git hook."""
    lines = text.splitlines(keepends=True)
    out: list[str] = []
    i = 0
    while i < len(lines):
        if marker in lines[i]:
            i += 1
            # Skip blank lines immediately after the marker, then the map command.
            while i < len(lines) and not lines[i].strip():
                i += 1
            if i < len(lines) and ("map --if-stale" in lines[i] or "map --no-wiki" in lines[i]):
                i += 1
            continue
        out.append(lines[i])
        i += 1
    return "".join(out)


def _uninstall_git_map_hooks(project_root: Path) -> list[str]:
    """Reverse ``_install_git_map_hooks`` — remove DevCouncil map-refresh blocks only."""
    root = project_root.expanduser().resolve()
    hooks_dir = root / ".git" / "hooks"
    if not hooks_dir.is_dir():
        return []
    removed: list[str] = []
    for name in ("post-commit", "post-merge", "post-checkout"):
        path = hooks_dir / name
        if not path.exists():
            continue
        existing = path.read_text(encoding="utf-8")
        if GIT_MAP_HOOK_MARKER not in existing:
            continue
        updated = _strip_git_map_hook_block(existing)
        rel = str(path.relative_to(root)) if path.is_relative_to(root) else str(path)
        # Drop file when only a shebang / whitespace remains.
        residual = updated.strip()
        if residual in ("", "#!/bin/sh", "#!/usr/bin/env bash", "#!/bin/bash"):
            path.unlink()
            removed.append(f"deleted {rel}")
        else:
            path.write_text(updated, encoding="utf-8")
            removed.append(f"stripped DevCouncil map block from {rel}")
    return removed


_NATIVE_HOOK_UNINSTALLERS = {
    "claude": _uninstall_claude_hooks,
    "codex": _uninstall_codex_hooks,
    "gemini": _uninstall_gemini_hooks,
    "cursor": _uninstall_cursor_hooks,
    "grok": _uninstall_grok_hooks,
    "opencode": _uninstall_opencode_hooks,
}


def _uninstall_native_hooks_for_tool(project_root: Path, tool: str) -> list[str]:
    """Uninstall DevCouncil hooks for one native hook tool (no git map hooks)."""
    uninstall = _NATIVE_HOOK_UNINSTALLERS.get(tool)
    if uninstall is None:
        raise ValueError(f"Unsupported hook tool '{tool}'.")
    return uninstall(project_root)


def _uninstall_all_native_hooks(project_root: Path, *, include_git: bool = True) -> list[str]:
    """Uninstall hook files/entries for every hook tool (+ optional git map hooks)."""
    removed: list[str] = []
    for uninstall in _NATIVE_HOOK_UNINSTALLERS.values():
        removed.extend(uninstall(project_root))
    if include_git:
        removed.extend(_uninstall_git_map_hooks(project_root))
    return removed

