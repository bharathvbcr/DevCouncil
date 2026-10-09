import functools


def helper(x=0):
    return x + 1


def split(text):
    """C20 collides by name with other/dup.py::split."""
    return text.split(",")


def score(row):
    return row * 2


def traced(fn):
    """C10 decorator using functools.wraps."""
    @functools.wraps(fn)
    def wrapper(*args, **kwargs):
        log_call(fn.__name__)
        return fn(*args, **kwargs)
    return wrapper


def log_call(name):
    return name


def make_adder(n):
    """C13 closure."""
    def add(x):
        return helper(x) + n
    return add


def apply(cb, x):
    """C15 callback invoked through a parameter."""
    return cb(x)


@traced
def work():
    return helper(3)
