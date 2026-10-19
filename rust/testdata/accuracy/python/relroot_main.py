from .relroot_util import rz_root_helper


def rz_root_run() -> int:
    """A root-level relative import: the importing package is the root itself."""
    return rz_root_helper()
