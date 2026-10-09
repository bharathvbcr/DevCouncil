from __future__ import annotations


class Base:
    def __init__(self, n: int) -> None:
        self.n = n

    def run(self) -> int:
        return self.n

    def quack(self) -> str:
        return "base"


class Engine(Base):
    """C07 inheritance + super(); C08 property; C09 classmethod/staticmethod; C21 dunders; C19 async."""

    def __init__(self, n: int) -> None:
        super().__init__(n)
        self.history: list[int] = []

    @property
    def size(self) -> int:
        return len(self.history)

    @classmethod
    def create(cls) -> "Engine":
        return cls(0)

    @staticmethod
    def norm(v: int) -> int:
        return abs(v)

    def step(self) -> int:
        out = self.norm(self.run())
        self.history.append(out)
        return out

    def run(self) -> int:
        return super().run() + 1

    def __call__(self) -> int:
        return self.step()

    def __enter__(self) -> "Engine":
        return self

    def __exit__(self, *exc) -> None:
        self.reset()

    def reset(self) -> None:
        self.history.clear()

    async def fetch(self) -> int:
        return await self.load()

    async def load(self) -> int:
        return self.n


class Duck:
    def quack(self) -> str:
        return "duck"
