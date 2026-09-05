"""What importing `devcouncil.app.errors` drags in behind it.

`devcouncil/app/errors.py` is six `class X(Exception): pass` and imports
nothing: **163 us** to execute. Importing it cost **304 ms** over a bare
interpreter, because `devcouncil/app/__init__.py` eagerly re-exported
`StateMachine`, `RunContext`, `Orchestrator` and `EventBus`, and reaching
`Orchestrator` loads `devcouncil.storage.db`, which loads SQLAlchemy and
SQLModel — **284.8 ms** of ORM that an exception class has no use for.

Every CLI invocation paid it: `import devcouncil.cli.main` was 582 ms over
bare, and `devcouncil.cli.commands.artifacts` reaches this chain through
`devcouncil.app.errors`. On a `dev map` whose kernel work is ~1 s, that is not
a rounding error.

The facade is kept rather than deleted. One caller in the whole tree imports a
name through it (`tests/conftest.py`), which is weak evidence for deletion and
none at all for the reflective and `getattr` paths a static search cannot see —
so PEP 562 makes it lazy, and every existing import keeps working.
"""

from __future__ import annotations

import subprocess
import sys


def _modules_after(statement: str) -> set[str]:
    """Top-level module names loaded by `statement` in a fresh interpreter.

    A subprocess, not `importlib.reload`: the point is what a *cold* import
    pulls in, and this test process has already imported most of the tree.
    """
    probe = subprocess.run(
        [
            sys.executable,
            "-c",
            f"{statement}\n"
            "import json, sys\n"
            "print(json.dumps(sorted({name.split('.')[0] for name in sys.modules})))",
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    import json

    return set(json.loads(probe.stdout))


def test_importing_an_exception_class_does_not_load_the_orm():
    """The regression this guards is a 285 ms import, not a wrong answer.

    Asserting on `sys.modules` rather than on elapsed time on purpose: a timing
    assertion is flaky on a loaded machine and says nothing about *why* it got
    slow. The ORM being absent is the same fact, checked exactly.
    """
    loaded = _modules_after("import devcouncil.app.errors")
    assert "sqlalchemy" not in loaded, (
        "importing six exception classes must not load SQLAlchemy; "
        "`devcouncil/app/__init__.py` is re-exporting eagerly again"
    )
    assert "sqlmodel" not in loaded


def test_the_package_facade_still_resolves_every_name_it_advertises():
    """Lazy must not mean absent.

    `__all__` is the advertised surface, so each name in it is fetched — if
    `__getattr__` ever stops covering one, this fails on that name rather than
    on whichever one a caller happened to reach first.
    """
    import devcouncil.app as app

    for name in app.__all__:
        assert getattr(app, name) is not None, f"{name} is advertised but does not resolve"

    from devcouncil.app import DevCouncilError, Orchestrator
    from devcouncil.app.errors import DevCouncilError as DirectError

    assert DevCouncilError is DirectError, "the facade must not create a second class object"
    assert Orchestrator.__name__ == "Orchestrator"


def test_an_unknown_attribute_is_still_an_attribute_error():
    """`__getattr__` must not turn a typo into a silent `None`."""
    import devcouncil.app as app

    try:
        app.NoSuchName
    except AttributeError as error:
        assert "NoSuchName" in str(error)
    else:  # pragma: no cover - the assertion above is the contract
        raise AssertionError("an unknown attribute must raise AttributeError")


def test_the_event_bus_is_one_object_however_it_is_reached():
    """The one module-level side effect in the package is a singleton.

    `devcouncil/app/events.py` builds `bus = EventBus()` at import. Under lazy
    loading it is built on first touch instead of at package import, so the
    property that matters is that both paths still reach the *same* object —
    two buses would silently split every subscription.
    """
    from devcouncil.app import bus as facade_bus
    from devcouncil.app.events import bus as direct_bus

    assert facade_bus is direct_bus
