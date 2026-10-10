import functools

from pkg.core import Duck, Engine
from pkg.util import apply, helper, make_adder, score, work


def typed(e: Engine) -> int:
    """C12 typed receiver via annotation."""
    return e.step()


def constructed() -> int:
    """C12 receiver typed by constructor assignment; inherited method via MRO."""
    x = Engine(1)
    x.reset()
    return x.run()


def inherited_only() -> str:
    """C07 method found only on the base class."""
    return Engine(1).quack()


def duck(o) -> str:
    """C14 duck-typed call: two classes declare quack -> must not be a confident edge."""
    return o.quack()


def with_ctx() -> int:
    """C21 context manager + __call__."""
    with Engine(1) as eng:
        return eng()


def partials() -> int:
    """C13/C15 functools.partial and callback by reference."""
    p = functools.partial(helper, 1)
    return p() + apply(helper, 2)


def lambdas(rows):
    """C13 lambda body calls score."""
    return sorted(rows, key=lambda r: score(r))


def closures() -> int:
    return make_adder(1)(2) + work()


async def runner() -> int:
    """C19 await on a typed receiver."""
    e = Engine(1)
    return await e.fetch()


def props() -> int:
    """C08 property access (no call syntax)."""
    return Engine(1).size


def statics() -> int:
    """C09 classmethod/staticmethod via class."""
    return Engine.norm(-1) + Engine.create().run()


def ducks_used() -> str:
    return Duck().quack()
