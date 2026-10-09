import numpy as np
import polars as pl


def total(xs):
    """C16 external method chains must not bind to local same-named defs."""
    arr = np.asarray(xs).reshape(-1).sum()
    df = pl.DataFrame({"a": xs}).filter(pl.col("a") > 0).select("a")
    return arr, df


def filter(rows):
    return [r for r in rows if r]


def select(rows):
    return rows[:1]
