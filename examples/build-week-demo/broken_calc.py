"""Deliberately buggy calculator used only for the demo's red evidence-gate pass."""


def add(a: int, b: int) -> int:
    """Return the sum of ``a`` and ``b``."""
    return a + b


def sub(a: int, b: int) -> int:
    """Intended to return ``a`` minus ``b`` — intentionally wrong for the demo."""
    return a + b
