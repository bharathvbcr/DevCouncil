"""Event-framework matchers with explicit registration operations."""

from __future__ import annotations

from collections.abc import Iterator

from devcouncil.codeintel.resolution.frameworks.base import (
    EventMatch,
    FrameworkSpec,
    compile_pattern,
)

EVENT_SPECS = (
    FrameworkSpec("node-event-emitter", "event", compile_pattern(
        r"\.(?P<operation>emit)\(\s*['\"](?P<event>[^'\"]+)['\"]"
    )),
    FrameworkSpec("react-native-event-emitter", "event", compile_pattern(
        r"\.(?P<operation>sendEventWithName|sendEvent)"
        r"\(\s*['\"](?P<event>[^'\"]+)['\"]"
    )),
    FrameworkSpec("node-event-listener", "event", compile_pattern(
        r"\.(?P<operation>on|once)\(\s*['\"](?P<event>[^'\"]+)['\"]"
        r"\s*,\s*(?P<callback>[A-Za-z_$][\w$]*)"
    )),
    FrameworkSpec("dom-event-listener", "event", compile_pattern(
        r"\.(?P<operation>addListener|addEventListener)"
        r"\(\s*['\"](?P<event>[^'\"]+)['\"]"
        r"\s*,\s*(?P<callback>[A-Za-z_$][\w$]*)"
    )),
    FrameworkSpec("watchdog-observer", "event", compile_pattern(
        r"\.(?P<operation>schedule)\(\s*(?P<callback>[A-Za-z_$][\w$]*)"
    )),
)


def iter_event_matches(line: str) -> Iterator[EventMatch]:
    for spec in EVENT_SPECS:
        for match in spec.pattern.finditer(line):
            groups = match.groupdict()
            yield EventMatch(
                operation=(groups.get("operation") or "").lower(),
                event=groups.get("event") or spec.name,
                callback_expression=groups.get("callback") or "",
                start=match.start(),
            )
