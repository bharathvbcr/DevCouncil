"""Manifest for independently testable framework semantic augmenters."""

from devcouncil.codeintel.resolution.frameworks.base import FrameworkSpec
from devcouncil.codeintel.resolution.frameworks.di import (
    DI_SPECS,
    iter_provider_matches,
)
from devcouncil.codeintel.resolution.frameworks.events import (
    EVENT_SPECS,
    iter_event_matches,
)
from devcouncil.codeintel.resolution.frameworks.routes import (
    COMPUTED_ROUTE_PATTERN,
    ROUTE_SPECS,
    iter_route_matches,
)

FRAMEWORK_MANIFEST: tuple[FrameworkSpec, ...] = (
    *ROUTE_SPECS,
    *DI_SPECS,
    *EVENT_SPECS,
)

__all__ = [
    "COMPUTED_ROUTE_PATTERN",
    "DI_SPECS",
    "EVENT_SPECS",
    "FRAMEWORK_MANIFEST",
    "ROUTE_SPECS",
    "iter_event_matches",
    "iter_provider_matches",
    "iter_route_matches",
]
