class Literal:
    """SH1 an in-repo class that shares its name with typing.Literal."""


class Protocol:
    """SH1 an in-repo class that shares its name with typing.Protocol."""


def shadowed_target():
    """SH2 the only in-repo function of this name."""
    return 1


class Callable:
    """SH1 an in-repo class that shares its name with collections.abc.Callable."""


class ndarray:
    """ST1 an in-repo class that shares its name with numpy.ndarray."""


def zeros(n):
    """ST1 the only in-repo `zeros`; a star import of numpy is where a bare `zeros` comes from."""
    return [0] * n


def ones(n):
    """ST3 one of two in-repo `ones`; the global tier abstains between them."""
    return [1] * n
