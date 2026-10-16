from .base import ZqBase


def build_a():
    class ZqNan(ZqBase):
        def accumulate(self, n):
            return super().accumulate(n)

    return ZqNan()
