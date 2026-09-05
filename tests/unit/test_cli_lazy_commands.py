"""What `dev <command>` imports, and what it must not.

`dev hook post-tool-use` runs on **every tool call** an agent makes and cost
408-493 ms measured, of which `devcouncil/cli/main.py` importing all ~50 command
modules was the dominant part: 23 of them pull `devcouncil.storage.db`, which
pulls SQLAlchemy and SQLModel. A hook that resolves no active task never touches
the database, so that was ~120 ms of ORM loaded to not be used, once per tool
call.

Commands are now built on first use. These tests pin both halves of that: the
laziness is real, and the command surface did not shrink to get it.
"""

from __future__ import annotations

import json
import subprocess
import sys

import typer

from devcouncil.cli.main import app

#: Every command `dev` exposed before commands were made lazy.
#:
#: Pinned as a literal, not derived from `app`, because deriving it from the
#: thing under test would make this assertion vacuous — a registration dropped
#: by the lazy rewrite would disappear from both sides at once.
COMMANDS_BEFORE = ["agents", "apply-patch", "approve", "artifacts", "ast", "attach-committed-range", "baseline", "boot", "campaign", "check", "checkout", "config", "corpus", "cost", "dashboard", "debug", "design", "doctor", "e2e", "evidence", "evidence-append", "evidence-list", "export", "gaps", "go", "graph", "graph-context", "handoff", "handoff-leased", "hook", "init", "integrate", "integrations", "lease", "logs", "lsp", "map", "mcp-server", "next-task", "okf", "optimize", "plan", "policy-check", "prompt", "provenance", "record-command", "release", "repair", "report", "requirements", "reset-demo-state", "resource", "rollback", "run", "run-cmd", "runs", "scaffold-ci", "scope", "semantic", "setup", "shell", "show", "skills", "status", "tasks", "trace", "verify", "verify-leased", "version", "watch", "wiki", "write"]


def _modules_after(statement: str) -> set[str]:
    """Top-level modules loaded by `statement` in a fresh interpreter."""
    probe = subprocess.run(
        [
            sys.executable,
            "-c",
            f"{statement}\n"
            "import json, sys\n"
            # To stderr: the command under test writes its own output to
            # stdout, and interleaving the two made this probe's JSON
            # unparseable rather than making the assertion fail.
            "print(json.dumps(sorted(sys.modules)), file=sys.stderr)",
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    return set(json.loads(probe.stderr.strip().splitlines()[-1]))


def test_the_command_surface_is_unchanged():
    """Laziness must not lose a command.

    This is the assertion that makes the rest safe to trust: `--help` and every
    documented invocation still resolve to something.
    """
    import click

    group = typer.main.get_command(app)
    # `list_commands`, not `.commands`: the latter is only what has been *built*
    # so far, and asserting on it would pass trivially by building nothing.
    # `list_commands` is what Click itself asks for help and completion, so it
    # is the surface a user sees.
    names = group.list_commands(click.Context(group, info_name="dev"))
    assert sorted(names) == COMMANDS_BEFORE, (
        "the lazy rewrite changed which commands exist; added: "
        f"{sorted(set(names) - set(COMMANDS_BEFORE))}, removed: "
        f"{sorted(set(COMMANDS_BEFORE) - set(names))}"
    )


def test_running_one_command_does_not_import_the_others():
    """The point of the change, stated as an invariant rather than a timing.

    A timing assertion would be flaky on a loaded machine and would not say
    *why* it got slow. "`dev hook` did not import the artifacts command" is the
    same fact, checked exactly.
    """
    loaded = _modules_after(
        "import sys; sys.argv = ['dev', 'hook', '--help']\n"
        "import contextlib, io\n"
        "from devcouncil.cli.main import app\n"
        "try:\n"
        "    with contextlib.redirect_stdout(io.StringIO()):\n"
        "        app()\n"
        "except SystemExit:\n"
        "    pass"
    )
    assert "devcouncil.cli.commands.hook" in loaded, "the invoked command must load"
    for unrelated in (
        "devcouncil.cli.commands.artifacts",
        "devcouncil.cli.commands.campaign",
        "devcouncil.cli.commands.dashboard",
    ):
        assert unrelated not in loaded, f"{unrelated} was imported for an unrelated command"


def test_a_command_that_needs_no_database_does_not_load_the_orm():
    """The ORM is loaded when a command uses it, and not otherwise.

    Stated against `dev version` rather than `dev hook` on purpose. The hook
    path *does* need the database — `active_task_id` opens it as its first act,
    to decide whether a single task is running — so a test asserting the hook
    runs ORM-free would be asserting a false premise, and could only be made to
    pass by breaking the hook. `dev version` is the honest case: nothing about
    it touches storage, and before commands were lazy it paid ~120 ms of
    SQLAlchemy anyway because some *other* command module imported it.

    Measured, minimum of 5, same machine: `dev version` 434 ms -> 75 ms.
    """
    loaded = _modules_after(
        "import sys; sys.argv = ['dev', 'version', '--help']\n"
        "import contextlib, io\n"
        "from devcouncil.cli.main import app\n"
        "try:\n"
        "    with contextlib.redirect_stdout(io.StringIO()):\n"
        "        app()\n"
        "except SystemExit:\n"
        "    pass"
    )
    assert "devcouncil.cli.commands.version" in loaded, "the invoked command must load"
    assert "sqlalchemy" not in loaded, (
        "a command that touches no storage must not load SQLAlchemy; some other "
        "command module is being imported eagerly again"
    )
    assert "sqlmodel" not in loaded


def test_the_map_command_does_not_load_the_orm_to_be_listed():
    """`dev map` reaches storage for one existence check, inside one function.

    At module scope that import was paid by every invocation — measured
    `dev map --help` 461 ms -> 237 ms once deferred. The command still works:
    the suite's own `dev map` tests exercise the deferred path.
    """
    loaded = _modules_after("import devcouncil.cli.commands.map")
    assert "sqlalchemy" not in loaded
    assert "sqlmodel" not in loaded


def test_every_command_still_builds():
    """`--help` walks the whole surface, so a command that cannot be imported
    fails here rather than the first time a user reaches for it."""
    group = typer.main.get_command(app)
    import click

    ctx = click.Context(group, info_name="dev")
    for name in COMMANDS_BEFORE:
        assert group.get_command(ctx, name) is not None, f"{name} no longer resolves"
