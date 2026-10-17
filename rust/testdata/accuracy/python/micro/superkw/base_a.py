class SkBase:
    def ping(self):
        return "a"

    @classmethod
    def build(cls):
        return cls()


class SkLeft:
    def hop(self):
        return "left"


class SkRight:
    def hop(self):
        return "right"


class SkSilent:
    def other(self):
        return None
