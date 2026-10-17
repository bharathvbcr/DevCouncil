import external_lib

from .engines import FiEngine, FiOther, make_engine, make_untyped


class FiCross:
    def __init__(self):
        self.engine = FiEngine()

    def go(self):
        return self.engine.run()


class FiSame:
    def go(self):
        self.engine = FiEngine()
        return self.engine.run()


class FiFactory:
    def __init__(self):
        self.engine = make_engine()

    def go(self):
        return self.engine.run()


class FiLeakA:
    def __init__(self):
        self.engine = FiEngine()


class FiLeakB:
    def go(self):
        return self.engine.run()


class FiTwice:
    def __init__(self):
        self.engine = FiEngine()

    def reset(self):
        self.engine = FiOther()

    def go(self):
        return self.engine.run()


class FiAnnotated:
    engine: FiOther

    def __init__(self):
        self.engine = FiEngine()

    def go(self):
        return self.engine.run()


class FiExternal:
    def __init__(self):
        self.engine = external_lib.build()

    def go(self):
        return self.engine.run()


class FiUntyped:
    def __init__(self):
        self.engine = make_untyped()

    def go(self):
        return self.engine.run()


class FiWrapped:
    def __init__(self):
        self.engine = [FiEngine()]

    def go(self):
        return self.engine.run()
