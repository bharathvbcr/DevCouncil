"""Both directions of the dead-symbol verdict, in one non-entry module.

A corpus of only-live symbols measures precision and cannot measure recall; one
of only-dead symbols measures the reverse. This module is deliberately both.
"""

from helpers import used_helper

__all__ = ["Api"]


class Api:
    def public_op(self):
        """LIVE: public member of an `__all__`-exported class.

        Nothing in the corpus calls it, and deleting it would still break any
        consumer of the declared public API.
        """
        return used_helper()


def _hidden():
    """Skipped: the leading underscore is Python's privacy convention."""
    return 1


def orphan_function():
    """DEAD: module-level, not in `__all__`, and nothing calls it."""
    return 2


def cycle_a():
    """DEAD: reachable only from `cycle_b`, which is itself unreachable.

    The abandoned-cycle shape. Each half has an inbound call edge, so a
    naive "no inbound edges" rule keeps both alive forever.
    """
    return cycle_b()


def cycle_b():
    """DEAD: the other half of the abandoned cycle."""
    return cycle_a()
