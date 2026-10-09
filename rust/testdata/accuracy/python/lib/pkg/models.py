from dataclasses import dataclass, field

from pydantic import BaseModel


@dataclass(frozen=True)
class Config:
    """C11 dataclass."""
    name: str
    tags: list[str] = field(default_factory=list)

    def validate(self) -> bool:
        return bool(self.name)


class Item(BaseModel):
    """C11 pydantic model."""
    id: int

    def label(self) -> str:
        return f"item-{self.id}"


def build_config() -> Config:
    cfg = Config(name="a")
    cfg.validate()
    return cfg
