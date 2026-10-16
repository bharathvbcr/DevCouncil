from .base import ZqBaseB


class ZqNan(ZqBaseB):
    def accumulate(self, n):
        return super().accumulate(n)


def use_b(step: ZqNan):
    return step.accumulate(1)
