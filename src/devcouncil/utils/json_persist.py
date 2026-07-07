"""Central JSON read/write helpers built on atomic fsio writes."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any, TypeVar, Union

from pydantic import BaseModel

from devcouncil.utils.fsio import atomic_write_json, atomic_write_text

PathLike = Union[str, Path]
ModelT = TypeVar("ModelT", bound=BaseModel)


def write_json(path: PathLike, payload: Any, *, indent: int = 2, sort_keys: bool = False) -> None:
    """Atomically serialize ``payload`` to JSON at ``path``."""
    atomic_write_json(path, payload, indent=indent, sort_keys=sort_keys)


def write_model_json(
    path: PathLike,
    model: BaseModel,
    *,
    indent: int = 2,
    exclude_none: bool = False,
) -> None:
    """Atomically write a Pydantic model as JSON."""
    atomic_write_text(
        path,
        model.model_dump_json(indent=indent, exclude_none=exclude_none) + "\n",
    )


def read_json(path: PathLike) -> Any:
    """Load JSON from ``path``; raises ``FileNotFoundError`` / ``json.JSONDecodeError``."""
    return json.loads(Path(path).read_text(encoding="utf-8"))


def read_model_json(path: PathLike, model_type: type[ModelT]) -> ModelT:
    """Load and validate JSON as a Pydantic model."""
    return model_type.model_validate_json(Path(path).read_text(encoding="utf-8"))


def dump_json(payload: Any, *, indent: int | None = None, sort_keys: bool = False, separators: tuple[str, str] | None = None) -> str:
    """Serialize ``payload`` to a JSON string (stdout / DB columns)."""
    if separators is not None:
        return json.dumps(payload, indent=indent, sort_keys=sort_keys, separators=separators)
    return json.dumps(payload, indent=indent, sort_keys=sort_keys)
