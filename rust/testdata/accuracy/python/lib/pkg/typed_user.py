from collections.abc import Callable
from typing import Literal, Protocol


def pick(mode: Literal["a", "b"]) -> str:
    return mode


class Sized(Protocol):
    def size(self) -> int: ...


def local_shadow():
    shadowed_target = lambda: 2
    return shadowed_target()


def run_cb(cb: Callable[[int], int]) -> int:
    return cb(1)
