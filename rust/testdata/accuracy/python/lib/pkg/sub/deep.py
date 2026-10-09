import pkg.util
from pkg import util as u
from pkg import Engine
from ..util import helper as h
from ..core import Engine as E2


def forms() -> int:
    """C02-C05 import forms."""
    a = pkg.util.helper(1)     # import a.b.c then dotted call
    b = u.helper(2)            # from pkg import util as u
    c = h(3)                   # relative + alias
    d = Engine(1).run()        # __init__ re-export
    e = E2(1).step()           # relative alias of class
    return a + b + c + d + e
