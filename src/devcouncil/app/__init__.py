"""The application package facade, resolved on first touch.

These names used to be imported eagerly here. `devcouncil/app/errors.py` is six
`class X(Exception): pass` and imports nothing — 163 us to execute — but
importing it cost **304 ms** over a bare interpreter, because reaching
`Orchestrator` from this file loads `devcouncil.storage.db`, which loads
SQLAlchemy and SQLModel: **284.8 ms** of ORM that an exception class has no use
for. Every CLI invocation paid it, `dev map` included, where it was comparable
to the kernel's own work.

The facade is kept rather than deleted. Exactly one caller in the tree imports
a name through it, which is weak evidence for deletion and none at all for the
reflective paths a static search cannot see — so PEP 562 defers the import to
the first attribute access and every existing import keeps working.

`TYPE_CHECKING` re-declares the same names for type checkers, which do not run
`__getattr__`; without it this file would type as `Any` and every annotation
built on these classes would stop being checked.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:  # pragma: no cover - import-time only for type checkers
    from devcouncil.app.errors import (
        ConfigurationError,
        DevCouncilError,
        ExecutionError,
        GatingError,
        OrchestrationError,
        VerificationError,
    )
    from devcouncil.app.events import EventBus, EventTypes, bus
    from devcouncil.app.orchestrator import Orchestrator
    from devcouncil.app.run_context import RunContext
    from devcouncil.app.state_machine import ProjectPhase, StateMachine

__all__ = [
    "StateMachine",
    "ProjectPhase",
    "RunContext",
    "Orchestrator",
    "EventBus",
    "bus",
    "EventTypes",
    "DevCouncilError",
    "GatingError",
    "ConfigurationError",
    "OrchestrationError",
    "ExecutionError",
    "VerificationError",
]

#: Advertised name -> the submodule that owns it.
#:
#: A table rather than a chain of `if name == ...`: `__all__` and this mapping
#: are checked against each other by
#: `test_the_package_facade_still_resolves_every_name_it_advertises`, so a name
#: added to one and not the other fails on that name instead of on whichever
#: caller reached it first.
_OWNERS: dict[str, str] = {
    "StateMachine": "devcouncil.app.state_machine",
    "ProjectPhase": "devcouncil.app.state_machine",
    "RunContext": "devcouncil.app.run_context",
    "Orchestrator": "devcouncil.app.orchestrator",
    "EventBus": "devcouncil.app.events",
    "bus": "devcouncil.app.events",
    "EventTypes": "devcouncil.app.events",
    "DevCouncilError": "devcouncil.app.errors",
    "GatingError": "devcouncil.app.errors",
    "ConfigurationError": "devcouncil.app.errors",
    "OrchestrationError": "devcouncil.app.errors",
    "ExecutionError": "devcouncil.app.errors",
    "VerificationError": "devcouncil.app.errors",
}


def __getattr__(name: str) -> Any:
    """Resolve an advertised name from its owning submodule, once.

    The resolved object is cached in this module's `globals()`, so the import
    machinery runs at most once per name and later accesses never reach here.

    An unadvertised name raises `AttributeError` rather than attempting an
    import: a typo must stay a typo, and a `__getattr__` that tried to import
    whatever it was handed would turn one into an arbitrary module load.
    """
    owner = _OWNERS.get(name)
    if owner is None:
        raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
    import importlib

    value = getattr(importlib.import_module(owner), name)
    globals()[name] = value
    return value


def __dir__() -> list[str]:
    """Keep tab-completion and `dir()` honest about the lazy names."""
    return sorted(set(globals()) | set(_OWNERS))
