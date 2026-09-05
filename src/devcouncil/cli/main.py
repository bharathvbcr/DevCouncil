import sys

import typer


def _configure_stdio() -> None:
    """Make stdout/stderr resilient to non-cp1252 characters.

    Coding agents and rich output emit Unicode such as ``✓``. On Windows the
    default console / redirected-pipe encoding is cp1252, where an un-encodable
    character raises UnicodeEncodeError mid-write. Because Rich buffers output,
    that error can surface during an unrelated later write — which previously
    got misreported as a coding agent "failing to start". Reconfigure both
    streams to UTF-8 with replacement so output can never crash the process.
    """
    for stream in (sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is None:
            continue
        try:
            reconfigure(encoding="utf-8", errors="replace")
        except (ValueError, OSError):
            try:
                reconfigure(errors="replace")
            except (ValueError, OSError):
                pass


_configure_stdio()

import importlib  # noqa: E402 - imports follow stdio reconfiguration

import typer
from typer.core import TyperGroup

#: `dev <name>` -> (module under `devcouncil.cli.commands`, attribute, kind).
#:
#: `kind` is `"typer"` for an attribute that is a `typer.Typer` (registered as a
#: sub-command group) and `"command"` for a plain function (registered as one
#: command).
#:
#: **Why a table instead of ~75 import-and-register statements.** `dev hook
#: post-tool-use` runs on every tool call an agent makes and measured 408-493 ms,
#: nearly all of it importing the other 49 command modules. 23 of them pull
#: `devcouncil.storage.db`, so every hook loaded SQLAlchemy and SQLModel — about
#: 120 ms — to answer a question that, with no single task running, never reaches
#: the database at all.
#:
#: The names here are the CLI's public surface and
#: `tests/unit/test_cli_lazy_commands.py` pins the full list against what it was
#: before this table existed, so a typo removes a command loudly rather than
#: silently.
_LAZY_COMMANDS: dict[str, tuple[str, str, str]] = {
    # Sub-command groups.
    "init": ("init", "app", "typer"),
    "doctor": ("doctor", "app", "typer"),
    "tasks": ("tasks", "app", "typer"),
    "report": ("report", "app", "typer"),
    "rollback": ("rollback", "app", "typer"),
    "config": ("config", "app", "typer"),
    "artifacts": ("artifacts", "app", "typer"),
    "agents": ("agents", "app", "typer"),
    "hook": ("hook", "app", "typer"),
    "version": ("version", "app", "typer"),
    "mcp-server": ("mcp_server", "app", "typer"),
    # `integrate` and `integrations` are the same sub-app under two names.
    "integrate": ("integrate", "app", "typer"),
    "integrations": ("integrate", "app", "typer"),
    "trace": ("trace", "app", "typer"),
    "logs": ("logs", "app", "typer"),
    "cost": ("cost", "app", "typer"),
    "runs": ("runs", "app", "typer"),
    "setup": ("setup", "app", "typer"),
    "lsp": ("lsp", "app", "typer"),
    "ast": ("ast", "app", "typer"),
    "dashboard": ("dashboard", "app", "typer"),
    "debug": ("debug_cmd", "app", "typer"),
    "watch": ("watch", "app", "typer"),
    "semantic": ("semantic", "app", "typer"),
    "evidence": ("evidence", "app", "typer"),
    "skills": ("skills", "app", "typer"),
    "okf": ("okf", "app", "typer"),
    "design": ("design", "app", "typer"),
    "wiki": ("wiki", "app", "typer"),
    "campaign": ("campaign", "app", "typer"),
    "resource": ("provenance", "resource_app", "typer"),
    "lease": ("lease", "lease_app", "typer"),
    "scope": ("task_gate", "scope_app", "typer"),
    # `map` and `graph` are the same sub-app under two names.
    "map": ("map", "app", "typer"),
    "graph": ("map", "app", "typer"),
    "corpus": ("graph_cmd", "corpus_app", "typer"),
    # Plain commands.
    "baseline": ("baseline", "baseline", "command"),
    "boot": ("boot", "boot", "command"),
    "e2e": ("go", "go", "command"),
    "go": ("go", "go", "command"),
    "graph-context": ("map", "graph_context_cmd", "command"),
    "scaffold-ci": ("scaffold", "scaffold_ci_command", "command"),
    "plan": ("plan", "plan", "command"),
    "approve": ("plan", "approve", "command"),
    "prompt": ("prompt", "prompt", "command"),
    "reset-demo-state": ("reset_demo_state", "reset_demo_state", "command"),
    "run": ("run", "run", "command"),
    # shell/handoff take a positional TASK_ID followed by options, so they must
    # be plain commands — as typer sub-apps (click groups) the documented
    # `dev shell TASK-001 --command ...` form fails to parse.
    "shell": ("shell", "shell", "command"),
    "handoff": ("handoff", "handoff", "command"),
    "show": ("show", "show", "command"),
    "verify": ("verify", "verify", "command"),
    "check": ("check", "check", "command"),
    "repair": ("repair", "repair", "command"),
    "status": ("status", "status", "command"),
    "gaps": ("gaps", "gaps", "command"),
    "provenance": ("provenance", "provenance", "command"),
    "checkout": ("lease", "checkout", "command"),
    "release": ("lease", "release", "command"),
    "write": ("gated_write", "write", "command"),
    "apply-patch": ("gated_write", "apply_patch", "command"),
    "next-task": ("task_gate", "next_task", "command"),
    "policy-check": ("task_gate", "policy_check", "command"),
    "record-command": ("task_gate", "record_command", "command"),
    "run-cmd": ("task_gate", "run_cmd", "command"),
    "attach-committed-range": ("task_gate", "attach_committed_range", "command"),
    "verify-leased": ("task_gate", "verify_leased", "command"),
    "evidence-append": ("task_gate", "evidence_append", "command"),
    "evidence-list": ("task_gate", "evidence_list", "command"),
    "handoff-leased": ("task_gate", "handoff_leased", "command"),
    "requirements": ("requirements", "requirements", "command"),
    "export": ("export", "export_state", "command"),
    "optimize": ("agents", "optimize_agent", "command"),
}


def _prepare_watch(sub_app: "typer.Typer") -> None:
    """`dev watch fs` lives in its own module and is attached to `watch`.

    Done here rather than at import time because there is no import time any
    more: the attachment has to happen when `watch` is built, and exactly once,
    which is what the built-command cache below guarantees.
    """
    from devcouncil.cli.commands.watch_fs import watch_fs

    sub_app.command("fs")(watch_fs)


#: Sub-apps needing a mutation before they are converted to a Click command.
_BEFORE_BUILD = {"watch": _prepare_watch}


class LazyCommandGroup(TyperGroup):
    """Import a command's module the first time that command is asked for.

    `list_commands` advertises every name without importing anything, so
    completion and the command list stay cheap. `get_command` is the only place
    that imports, and it caches the built command on the group, so a command is
    never built twice in one process.

    `dev --help` still renders every command's help, which means it still
    imports every module — correct, and rare. The hot path is
    `dev <one command>`, which now imports one.
    """

    def list_commands(self, ctx: "typer.Context") -> list[str]:
        return sorted({*super().list_commands(ctx), *_LAZY_COMMANDS})

    def get_command(self, ctx: "typer.Context", name: str):
        found = super().get_command(ctx, name)
        if found is not None:
            return found
        spec = _LAZY_COMMANDS.get(name)
        if spec is None:
            # Unknown name: Click's own "no such command" path, not a crash.
            return None
        module_name, attribute, kind = spec
        module = importlib.import_module(f"devcouncil.cli.commands.{module_name}")
        target = getattr(module, attribute)
        if kind == "typer":
            prepare = _BEFORE_BUILD.get(name)
            if prepare is not None:
                prepare(target)
            # `get_group`, never `get_command`. Typer collapses a sub-app that
            # holds exactly one command and no callback into a bare
            # `click.Command`, so `dev lsp inspect` parsed `inspect` as a
            # positional argument to `dev lsp` and failed. `add_typer` — what
            # the eager registration used — always builds a group, and this is
            # the call that reproduces it.
            command = typer.main.get_group(target)
        else:
            holder = typer.Typer()
            holder.command(name=name)(target)
            # The collapse is correct here and only here: a holder with one
            # command is exactly `app.command(...)`, a leaf that takes its own
            # arguments.
            command = typer.main.get_command(holder)
        command.name = name
        # Cache on the group so a second lookup in the same process — Click does
        # several while parsing — neither re-imports nor rebuilds.
        self.add_command(command, name)
        return command


app = typer.Typer(
    name="dev",
    help="DevCouncil: Gated orchestrator for AI-assisted software development.",
    add_completion=False,
    cls=LazyCommandGroup,
)


@app.callback()
def main(
    ctx: typer.Context,
    verbose: int = typer.Option(
        0,
        "--verbose",
        "-v",
        count=True,
        help="Increase console log verbosity (-v INFO, -vv DEBUG). Everything is "
        "always captured at DEBUG in .devcouncil/logs/devcouncil.log.",
    ),
    quiet: bool = typer.Option(
        False,
        "--quiet",
        "-q",
        help="Only show errors on the console (the log file still captures everything).",
    ),
    log_level: str = typer.Option(
        None,
        "--log-level",
        help="Explicit console log level (DEBUG/INFO/WARNING/ERROR). Overrides -v/-q "
        "and the DEVCOUNCIL_LOG_LEVEL env var.",
    ),
):
    """
    DevCouncil: Gated orchestrator for AI-assisted software development.
    """
    # Configure logging once, up front, for every command. Without this the many
    # logger.info/debug calls across the orchestrator, planner, executors and
    # verifier go nowhere — which is exactly why recurring run failures were so
    # hard to diagnose. The durable DEBUG log lands in .devcouncil/logs/.
    from devcouncil.telemetry.logging_setup import configure_logging

    configure_logging(verbosity=verbose, quiet=quiet, log_level=log_level)
    return

def run_cli() -> None:
    """Console-script entry: run the CLI with EPIPE-safe stdout.

    ``dev map | head`` (or any consumer closing the pipe early) must exit with
    the conventional SIGPIPE status instead of a BrokenPipeError traceback.
    """
    import os

    try:
        app()
    except BrokenPipeError:
        try:
            os.dup2(os.open(os.devnull, os.O_WRONLY), sys.stdout.fileno())
        except OSError:
            pass
        sys.exit(128 + 13)


if __name__ == "__main__":
    run_cli()
