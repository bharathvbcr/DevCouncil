"""Dependency-injection matcher families."""

from __future__ import annotations

from collections.abc import Iterator

from devcouncil.codeintel.resolution.frameworks.base import (
    FrameworkSpec,
    ProviderMatch,
    compile_pattern,
)

DI_SPECS = (
    FrameworkSpec("python-di", "dependency-injection", compile_pattern(
        r"\b(?:Depends|Inject)\s*\(\s*(?P<target>[A-Za-z_$][\w$.:]*)"
    )),
    FrameworkSpec("jvm-di", "dependency-injection", compile_pattern(
        r"\b(?:Autowired|Provides|Binds|Singleton)\s*\(\s*"
        r"(?P<target>[A-Za-z_$][\w$.:]*)"
    )),
    FrameworkSpec("dotnet-di", "dependency-injection", compile_pattern(
        r"\b(?:AddSingleton|AddScoped|AddTransient)(?:<(?P<generic>[^>]+)>)?"
        r"\s*\(\s*(?:new\s+)?(?P<target>[A-Za-z_$][\w$.:]*)?"
    )),
    FrameworkSpec("container-di", "dependency-injection", compile_pattern(
        r"\b(?:bind|singleton|factory)\s*\(\s*(?P<target>[A-Za-z_$][\w$.:]*)"
    )),
    FrameworkSpec("wire-di", "dependency-injection", compile_pattern(
        r"\bwire\.Build\s*\(\s*(?P<target>[A-Za-z_$][\w$.:]*)"
    )),
    FrameworkSpec("fx-di", "dependency-injection", compile_pattern(
        r"\bfx\.Provide\s*\(\s*(?P<target>[A-Za-z_$][\w$.:]*)"
    )),
)


def iter_provider_matches(line: str) -> Iterator[ProviderMatch]:
    for spec in DI_SPECS:
        for match in spec.pattern.finditer(line):
            groups = match.groupdict()
            generic_types = [
                value.strip()
                for value in (groups.get("generic") or "").split(",")
                if value.strip()
            ]
            target = (
                generic_types[-1]
                if generic_types
                else groups.get("target") or ""
            )
            if target:
                yield ProviderMatch(
                    framework=spec.name,
                    target_expression=target,
                    start=match.start(),
                )
