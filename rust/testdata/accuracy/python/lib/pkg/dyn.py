import importlib

from pkg import util


def by_name(name):
    """C22 getattr dispatch: honest miss."""
    return getattr(util, name)(1)


def by_import():
    """C22 importlib dispatch: honest miss / or module-resolved."""
    mod = importlib.import_module("pkg.util")
    return mod.helper(1)
