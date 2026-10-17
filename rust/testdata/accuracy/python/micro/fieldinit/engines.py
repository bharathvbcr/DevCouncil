class FiEngine:
    def run(self):
        return 1


class FiOther:
    def run(self):
        return 2


def make_engine() -> FiEngine:
    return FiEngine()


def make_untyped():
    return FiEngine()
