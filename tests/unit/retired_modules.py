"""One assertion for "this module is gone", with the diagnosis a stale checkout needs.

A package deleted from the tree stays importable on any checkout that still
holds its ``__pycache__``: a directory with no source is a namespace package,
so ``importlib.import_module`` succeeds and returns an empty module. That is
what every developer sees after pulling a deletion, and ``pytest.raises(
ModuleNotFoundError)`` reports it as ``DID NOT RAISE`` — true, and useless.
Measured on the branch that retired ``devcouncil.codeintel.sync``: two tests
red in the lead worktree, green in a fresh clone, for one leftover directory.
"""

from __future__ import annotations

import importlib

import pytest


def assert_module_is_gone(name: str) -> None:
    """Fail unless importing *name* raises ``ModuleNotFoundError``.

    When the import succeeds as an empty namespace package, the failure names
    the leftover directory and the command that removes it, so the reader can
    tell a stale checkout from a module that came back.
    """
    try:
        module = importlib.import_module(name)
    except ModuleNotFoundError:
        return
    origin = getattr(getattr(module, "__spec__", None), "origin", None)
    paths = [str(path) for path in getattr(module, "__path__", [])]
    if origin is None and paths:
        pytest.fail(
            f"{name} still imports, as an empty namespace package, from {paths}: a "
            "directory left behind by an older checkout (usually its __pycache__). "
            f"Remove it: rm -rf {' '.join(paths)}"
        )
    pytest.fail(f"{name} is back (imported from {origin}); it was retired with the Python graph store")
