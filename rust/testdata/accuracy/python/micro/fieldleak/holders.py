from .engines import FlEngine, FlOther


class FlHolderA:
    engine: FlEngine

    def go(self):
        return self.engine.run()


class FlHolderB:
    def __init__(self):
        self.engine = FlOther()

    def go(self):
        return self.engine.run()
