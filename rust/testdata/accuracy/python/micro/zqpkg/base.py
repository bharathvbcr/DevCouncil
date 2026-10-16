class ZqBase:
    def accumulate(self, n):
        return n


class ZqBaseB:
    def accumulate(self, n):
        return n + 1


class ZqTable:
    def digest_of(self):
        return "d"

    def cache_clear(self):
        return None


def make_table(keys) -> ZqTable:
    return ZqTable()


def unannotated_pair():
    table = ZqTable()
    other = ZqBase()
    return table, other
