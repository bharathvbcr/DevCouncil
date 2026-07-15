"""Shared records for isolated framework semantic augmenters."""

from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Pattern


@dataclass(frozen=True)
class FrameworkSpec:
    """A framework matcher advertised by the semantic resolver."""

    name: str
    family: str
    pattern: Pattern[str]


@dataclass(frozen=True)
class RouteMatch:
    framework: str
    verb: str
    path: str
    handler_expression: str
    start: int
    end: int


@dataclass(frozen=True)
class EventMatch:
    operation: str
    event: str
    callback_expression: str
    start: int


@dataclass(frozen=True)
class ProviderMatch:
    framework: str
    target_expression: str
    start: int


def compile_pattern(pattern: str, *, multiline: bool = False) -> Pattern[str]:
    flags = re.IGNORECASE | (re.MULTILINE if multiline else 0)
    return re.compile(pattern, flags)
