from .base import ZqBase


def build_c():
    class ZqUniq(ZqBase):
        def accumulate(self, n):
            return super().accumulate(n)

    return ZqUniq()
