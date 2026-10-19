from .util import rz_f, rz_g


def rz_missing_run() -> int:
    """pkg/util.py is absent: neither src/rzpkg/util.py nor rzpkg/rzpkg/util.py is this module."""
    return rz_f() + rz_g()
