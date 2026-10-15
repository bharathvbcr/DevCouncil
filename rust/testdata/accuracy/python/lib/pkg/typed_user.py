from typing import Literal, Protocol


def pick(mode: Literal["a", "b"]) -> str:
    return mode


class Sized(Protocol):
    def size(self) -> int: ...


def local_shadow():
    shadowed_target = lambda: 2
    return shadowed_target()
