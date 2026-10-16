from .base import ZqBaseB, ZqTable
from .trainer import ZqLog, ZqLogSlots


def anchor(a: ZqBaseB, t: ZqTable, l: ZqLog, s: ZqLogSlots):
    a.accumulate(1)
    t.digest_of()
    t.cache_clear()
    l.losses()
    return s.losses()
