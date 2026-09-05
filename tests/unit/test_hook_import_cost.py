"""What importing the hook command module drags in behind it.

`dev hook post-tool-use` runs once per tool call an agent makes, so importing
`devcouncil.cli.commands.hook` is on the hottest path in the system. It cost
**149 ms** measured with `python -X importtime`: `devcouncil.live.tasks` reached
`devcouncil.storage.db` and loaded SQLAlchemy and SQLModel (90 ms), and
`json_persist`, `hook_policy` and `telemetry.traces` between them loaded pydantic
(another 42 ms). The `PostToolUse` map path touches none of it — it decides from
the payload's file extensions whether a build is worth running and otherwise
returns — and the majority of tool calls are not code edits, so the majority of
hook invocations paid the whole import for nothing.

Asserting on `sys.modules` rather than on elapsed time, for the reason
`test_app_package_import_cost` gives: a timing assertion is flaky on a loaded
machine and says nothing about *why* it got slow. The heavy packages being
absent is the same fact, checked exactly.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path


def _modules_after(statement: str) -> set[str]:
    """Top-level module names loaded by *statement* in a fresh interpreter.

    A subprocess, not `importlib.reload`: the point is what a *cold* import
    pulls in, and this test process has already imported most of the tree.

    The probe is pinned to the `devcouncil` package *this* process imported. In
    a git worktree the interpreter's editable install points at the primary
    checkout, so a bare `python -c "import devcouncil…"` measures a different
    tree than the one under test — and reports its state as this one's.
    """
    import devcouncil

    source_root = Path(devcouncil.__file__).resolve().parent.parent
    env = dict(os.environ)
    env["PYTHONPATH"] = os.pathsep.join(
        [str(source_root), *([env["PYTHONPATH"]] if env.get("PYTHONPATH") else [])]
    )
    probe = subprocess.run(
        [
            sys.executable,
            "-c",
            f"{statement}\n"
            "import devcouncil, json, sys\n"
            "print(json.dumps({'root': devcouncil.__file__, "
            "'modules': sorted({name.split('.')[0] for name in sys.modules})}))",
        ],
        capture_output=True,
        text=True,
        check=True,
        env=env,
    )
    payload = json.loads(probe.stdout)
    assert Path(payload["root"]).resolve() == Path(devcouncil.__file__).resolve(), (
        "the probe imported a different checkout than the test process: "
        f"{payload['root']} vs {devcouncil.__file__}"
    )
    return set(payload["modules"])


def test_importing_the_hook_module_does_not_load_the_orm_or_pydantic():
    loaded = _modules_after("import devcouncil.cli.commands.hook")
    for heavy in ("sqlalchemy", "sqlmodel", "pydantic", "rich"):
        assert heavy not in loaded, (
            f"importing the hook module must not load {heavy}; a module-scope "
            "import of one of the deferred names is back"
        )


def test_every_deferred_name_still_resolves_and_is_still_patchable():
    """Lazy must not mean absent — and it must not mean unpatchable either.

    Both halves matter. `hook.get_db` and friends are what eleven tests
    substitute to drive the gate paths, so each has to be a real module
    attribute; and each has to resolve as a *global* inside the functions that
    call it, which a PEP 562 `__getattr__` entry does not — `__getattr__` is
    consulted for `module.name`, never for a bare name in a function body.
    """
    from devcouncil.cli.commands import hook

    for name in ("get_db", "active_task_id", "dump_json", "HookPolicy", "TraceLogger", "write_signal"):
        assert getattr(hook, name) is not None, f"{name} does not resolve"

    # Every deferred name except `get_db` is a real entry in the module dict,
    # which is what makes a bare global reference inside a function work.
    for name in ("active_task_id", "dump_json", "HookPolicy", "TraceLogger", "write_signal"):
        assert name in vars(hook), (
            f"{name} is not in the module dict, so every call site that spells it "
            "as a bare global will raise NameError at run time"
        )


def test_an_unknown_attribute_is_still_an_attribute_error():
    from devcouncil.cli.commands import hook

    try:
        hook.NoSuchName
    except AttributeError as error:
        assert "NoSuchName" in str(error)
    else:  # pragma: no cover - the assertion above is the contract
        raise AssertionError("an unknown attribute must raise AttributeError")


def test_the_lock_helper_can_serialize_without_a_module_scope_import():
    """The regression that a `__getattr__`-only deferral actually caused.

    `_try_acquire_refresh_lock` writes its pid through `dump_json` as a bare
    global. With `dump_json` deferred through the module `__getattr__` alone the
    call raised `NameError` — and `post_tool_use` swallows exceptions so the map
    refresh never ran and never said why.
    """
    import tempfile
    from pathlib import Path

    from devcouncil.cli.commands import hook

    with tempfile.TemporaryDirectory() as tmp:
        lock = Path(tmp) / "map_refresh.lock"
        assert hook._try_acquire_refresh_lock(lock) is True
        assert lock.is_file(), "the lock must carry the holder's pid"
        assert "pid" in lock.read_text(encoding="utf-8")
        # A second acquirer must not take a lock a live process holds.
        assert hook._try_acquire_refresh_lock(lock) is False
