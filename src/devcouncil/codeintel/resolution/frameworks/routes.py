"""Routing-framework matchers with no graph or liveness side effects."""

from __future__ import annotations

from collections.abc import Iterator

from devcouncil.codeintel.resolution.frameworks.base import (
    FrameworkSpec,
    RouteMatch,
    compile_pattern,
)

ROUTE_SPECS = (
    FrameworkSpec("decorator", "routing", compile_pattern(
        r"@(?:app|router|bp)\.(?P<verb>get|post|put|patch|delete|options|head)"
        r"\(\s*['\"](?P<path>[^'\"]+)['\"](?P<handler>[^)]*)"
    )),
    FrameworkSpec("django", "routing", compile_pattern(
        r"\b(?:path|re_path)\(\s*['\"](?P<path>[^'\"]+)['\"]"
        r"\s*,\s*(?P<handler>[^,)]+)"
    )),
    FrameworkSpec("spring", "routing", compile_pattern(
        r"@(?P<verb>Get|Post|Put|Patch|Delete|Request)Mapping"
        r"\(\s*(?:value\s*=\s*)?['\"](?P<path>[^'\"]+)['\"](?P<handler>[^)]*)"
    )),
    FrameworkSpec("nest", "routing", compile_pattern(
        r"@(?P<verb>Get|Post|Put|Patch|Delete|Options|Head|All)"
        r"\(\s*['\"](?P<path>[^'\"]*)['\"]?(?P<handler>[^)]*)"
    )),
    FrameworkSpec("aspnet", "routing", compile_pattern(
        r"\bMap(?P<verb>Get|Post|Put|Patch|Delete)"
        r"\(\s*['\"](?P<path>[^'\"]+)['\"]\s*,?\s*(?P<handler>[^)]*)"
    )),
    FrameworkSpec("aspnet", "routing", compile_pattern(
        r"\[Http(?P<verb>Get|Post|Put|Patch|Delete)"
        r"\(\s*['\"](?P<path>[^'\"]*)['\"]?(?P<handler>[^)]*)"
    )),
    FrameworkSpec("laravel", "routing", compile_pattern(
        r"\bRoute::(?P<verb>get|post|put|patch|delete|any)"
        r"\(\s*['\"](?P<path>[^'\"]+)['\"]\s*,?\s*(?P<handler>.*)"
    )),
    FrameworkSpec("router", "routing", compile_pattern(
        r"\b(?:app|router|r)\.(?P<verb>GET|POST|PUT|PATCH|DELETE|HandleFunc|route)"
        r"\(\s*['\"](?P<path>[^'\"]+)['\"]\s*,?\s*(?P<handler>[^)]*)"
    )),
    FrameworkSpec("rails", "routing", compile_pattern(
        r"^\s*(?P<verb>get|post|put|patch|delete)\s+['\"](?P<path>[^'\"]+)['\"]"
        r"(?P<handler>.*)"
    )),
    FrameworkSpec("axum", "routing", compile_pattern(
        r"\.route\(\s*['\"](?P<path>[^'\"]+)['\"]\s*,\s*"
        r"(?P<verb>get|post|put|patch|delete)\s*\(\s*(?P<handler>[^)]*)"
    )),
    FrameworkSpec("play", "routing", compile_pattern(
        r"^\s*(?P<verb>GET|POST|PUT|PATCH|DELETE)\s+(?P<path>/\S+)"
        r"\s+(?P<handler>\S+)"
    )),
    FrameworkSpec("react-router", "routing", compile_pattern(
        r"<Route\b[^>]*\bpath\s*=\s*['\"](?P<path>[^'\"]+)['\"]"
        r"(?P<handler>[^>]*)"
    )),
    FrameworkSpec("vue-router", "routing", compile_pattern(
        r"\{[^}\n]*\bpath\s*:\s*['\"](?P<path>/[^'\"]*)['\"]"
        r"(?P<handler>[^}\n]*)"
    )),
    FrameworkSpec("drupal", "routing", compile_pattern(
        r"^\s*path\s*:\s*['\"](?P<path>/[^'\"]*)['\"](?P<handler>.*)"
    )),
)

COMPUTED_ROUTE_PATTERN = compile_pattern(
    r"\b(?:app|router|bp|r)\.(?P<verb>get|post|put|patch|delete|route)"
    r"\(\s*(?P<expr>[^,]+),\s*(?P<handler>[^)]*)"
)


def iter_route_matches(line: str) -> Iterator[RouteMatch]:
    for spec in ROUTE_SPECS:
        for match in spec.pattern.finditer(line):
            groups = match.groupdict()
            yield RouteMatch(
                framework=spec.name,
                verb=(groups.get("verb") or "ANY").upper(),
                path=groups.get("path") or "",
                handler_expression=(groups.get("handler") or "").strip(),
                start=match.start(),
                end=match.end(),
            )
