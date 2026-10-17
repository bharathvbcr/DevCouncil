class SkBase:
    def ping(self):
        return "b"

    @classmethod
    def build(cls):
        return cls()
