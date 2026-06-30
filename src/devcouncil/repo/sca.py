"""Best-effort software-composition analysis (dependency vulnerability awareness).

DevCouncil does not bundle a vulnerability database and must never reach the
network on its own initiative. This module is a thin, *offline-safe* wrapper
around whatever auditors the developer already has installed (``pip-audit``,
``npm audit``, ``osv-scanner``): it detects the auditor + the project's
lockfiles, runs the local tool with a bounded timeout and a sanitized subprocess
environment, and parses the output into a structured list of dependency risks.

Design contract:
- **Never raises.** Every public entry point swallows tool/parse/OS errors and
  degrades to an empty result, so a missing tool or malformed output can never
  break ``dev map`` / prompt building / CI scaffolding.
- **Opt-in / local-only by default.** Nothing here runs unless a caller asks for
  it; ``ScaScanner.scan`` is the only thing that shells out.
- **Injectable runner.** ``ScaScanner`` takes an ``auditor_runner`` callable so
  tests (and offline environments) can feed canned auditor output without any
  network access or installed tools.
"""

from __future__ import annotations

import json
import shutil
import subprocess
from collections.abc import Callable
from dataclasses import asdict, dataclass
from pathlib import Path

from devcouncil.utils.subprocess_env import clean_subprocess_env

# Default per-auditor timeout. Bounded so a slow/hung auditor can't stall the map.
DEFAULT_TIMEOUT = 60


@dataclass(frozen=True)
class DependencyRisk:
    """A single dependency vulnerability finding (auditor-agnostic shape)."""

    package: str
    installed_version: str
    severity: str
    advisory_id: str
    summary: str

    def as_dict(self) -> dict[str, str]:
        return asdict(self)


@dataclass(frozen=True)
class AuditorResult:
    """Raw output of one auditor invocation; ``returncode`` < 0 means it didn't run."""

    returncode: int
    stdout: str
    stderr: str


# An auditor runner takes the auditor's argv and the project root, and returns the
# captured result. Injected so tests need no installed tools or network access.
AuditorRunner = Callable[[list[str], Path], AuditorResult]


@dataclass(frozen=True)
class _Auditor:
    name: str  # logical auditor name (pip-audit / npm / osv-scanner)
    executable: str  # the executable to look for on PATH
    stack: str  # which stack it covers (python / node)
    # Lockfiles whose presence makes this auditor relevant for the repo.
    lockfiles: tuple[str, ...]


# Ordered so the most specific / preferred auditor for each stack comes first.
_AUDITORS: tuple[_Auditor, ...] = (
    _Auditor(
        name="pip-audit",
        executable="pip-audit",
        stack="python",
        lockfiles=("uv.lock", "requirements.txt", "poetry.lock"),
    ),
    _Auditor(
        name="npm",
        executable="npm",
        stack="node",
        lockfiles=("package-lock.json", "yarn.lock", "pnpm-lock.yaml"),
    ),
    _Auditor(
        name="osv-scanner",
        executable="osv-scanner",
        stack="any",
        lockfiles=(
            "uv.lock",
            "requirements.txt",
            "poetry.lock",
            "package-lock.json",
            "yarn.lock",
            "pnpm-lock.yaml",
            "go.sum",
            "Cargo.lock",
        ),
    ),
)


def _default_runner(timeout: int) -> AuditorRunner:
    """Real subprocess runner: bounded timeout + sanitized env; never raises."""

    def run(argv: list[str], project_root: Path) -> AuditorResult:
        try:
            completed = subprocess.run(
                argv,
                cwd=project_root,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=timeout,
                env=clean_subprocess_env(),
            )
        except subprocess.TimeoutExpired:
            return AuditorResult(returncode=-1, stdout="", stderr="timed out")
        except (FileNotFoundError, OSError) as exc:
            return AuditorResult(returncode=-1, stdout="", stderr=str(exc))
        return AuditorResult(
            returncode=completed.returncode,
            stdout=completed.stdout or "",
            stderr=completed.stderr or "",
        )

    return run


class ScaScanner:
    """Detects locally-available auditors + lockfiles and runs them best-effort.

    Pass ``auditor_runner`` to inject the subprocess behaviour (tests do this to
    return canned auditor output offline). When omitted, a real bounded-timeout,
    clean-env subprocess runner is used.
    """

    def __init__(
        self,
        project_root: Path,
        *,
        auditor_runner: AuditorRunner | None = None,
        timeout: int = DEFAULT_TIMEOUT,
        which: Callable[[str], str | None] = shutil.which,
    ):
        self.project_root = Path(project_root)
        self.timeout = timeout
        self._which = which
        # When a runner is injected (tests / offline), the runner itself decides
        # whether a tool "exists"; we skip the PATH gate so canned output flows.
        self._injected = auditor_runner is not None
        self._runner: AuditorRunner = auditor_runner or _default_runner(timeout)
        # project_root is immutable per instance, so lockfile existence is stable;
        # memoize per filename to avoid repeated stat calls across auditors.
        self._lockfile_cache: dict[str, bool] = {}

    # -- detection -----------------------------------------------------------

    def _check_cached_lockfile(self, name: str) -> bool:
        cached = self._lockfile_cache.get(name)
        if cached is None:
            cached = (self.project_root / name).exists()
            self._lockfile_cache[name] = cached
        return cached

    def _has_lockfile(self, auditor: _Auditor) -> bool:
        return any(self._check_cached_lockfile(name) for name in auditor.lockfiles)

    def _is_runnable(self, auditor: _Auditor) -> bool:
        """Relevant to the repo (lockfile present) and runnable (on PATH or injected)."""
        if not self._has_lockfile(auditor):
            return False
        return self._injected or self._which(auditor.executable) is not None

    def available_auditors(self) -> list[str]:
        """Names of auditors that are both runnable AND relevant to this repo.

        An auditor counts as runnable when its executable is on PATH (real
        installs) OR when a custom runner is injected (tests/offline).
        """
        seen: set[str] = set()
        ordered: list[str] = []
        for auditor in _AUDITORS:
            if self._is_runnable(auditor) and auditor.name not in seen:
                seen.add(auditor.name)
                ordered.append(auditor.name)
        return ordered

    # -- scanning ------------------------------------------------------------

    def scan(self) -> list[DependencyRisk]:
        """Run every available auditor and merge their findings. Never raises."""
        risks: list[DependencyRisk] = []
        seen: set[tuple[str, str, str]] = set()
        for auditor in _AUDITORS:
            if not self._is_runnable(auditor):
                continue
            try:
                result = self._runner(self._argv_for(auditor), self.project_root)
                parsed = self._parse(auditor, result)
            except Exception:
                # Best-effort contract: a broken auditor/parse must not propagate.
                continue
            for risk in parsed:
                key = (risk.package, risk.installed_version, risk.advisory_id)
                if key in seen:
                    continue
                seen.add(key)
                risks.append(risk)
        return risks

    @staticmethod
    def _argv_for(auditor: _Auditor) -> list[str]:
        if auditor.name == "pip-audit":
            return ["pip-audit", "--format", "json"]
        if auditor.name == "npm":
            return ["npm", "audit", "--json"]
        if auditor.name == "osv-scanner":
            return ["osv-scanner", "--format", "json", "."]
        return [auditor.executable]

    def _parse(self, auditor: _Auditor, result: AuditorResult) -> list[DependencyRisk]:
        if result.returncode < 0 or not result.stdout.strip():
            return []
        try:
            data = json.loads(result.stdout)
        except (json.JSONDecodeError, ValueError):
            return []
        if auditor.name == "pip-audit":
            return _parse_pip_audit(data)
        if auditor.name == "npm":
            return _parse_npm_audit(data)
        if auditor.name == "osv-scanner":
            return _parse_osv_scanner(data)
        return []


# ----------------------------------------------------------------------------
# Per-auditor parsers. Each is defensive: unknown shapes yield no risks.
# ----------------------------------------------------------------------------


def _coerce_str(value: object, default: str = "") -> str:
    if value is None:
        return default
    if isinstance(value, str):
        return value
    return str(value)


def _parse_pip_audit(data: object) -> list[DependencyRisk]:
    """pip-audit ``--format json`` -> dependencies[].vulns[]."""
    risks: list[DependencyRisk] = []
    deps: object
    if isinstance(data, dict):
        deps = data.get("dependencies", [])
    elif isinstance(data, list):
        deps = data  # older pip-audit emitted a bare list
    else:
        return []
    if not isinstance(deps, list):
        return []
    for dep in deps:
        if not isinstance(dep, dict):
            continue
        name = _coerce_str(dep.get("name"))
        version = _coerce_str(dep.get("version"))
        vulns = dep.get("vulns") or []
        if not isinstance(vulns, list):
            continue
        for vuln in vulns:
            if not isinstance(vuln, dict):
                continue
            risks.append(
                DependencyRisk(
                    package=name,
                    installed_version=version,
                    severity=_coerce_str(vuln.get("severity"), "unknown") or "unknown",
                    advisory_id=_coerce_str(vuln.get("id"), "UNKNOWN") or "UNKNOWN",
                    summary=_coerce_str(vuln.get("description") or vuln.get("summary")),
                )
            )
    return risks


def _parse_npm_audit(data: object) -> list[DependencyRisk]:
    """npm audit ``--json`` (npm v7+) -> vulnerabilities{ name: {...} }."""
    if not isinstance(data, dict):
        return []
    vulnerabilities = data.get("vulnerabilities")
    if not isinstance(vulnerabilities, dict):
        return []
    risks: list[DependencyRisk] = []
    for name, info in vulnerabilities.items():
        if not isinstance(info, dict):
            continue
        severity = _coerce_str(info.get("severity"), "unknown") or "unknown"
        version = _coerce_str(info.get("range"))
        via = info.get("via") or []
        advisory_id = "UNKNOWN"
        summary = ""
        if isinstance(via, list):
            for entry in via:
                if isinstance(entry, dict):
                    source = entry.get("source") or entry.get("url")
                    advisory_id = _coerce_str(source, "UNKNOWN") or "UNKNOWN"
                    summary = _coerce_str(entry.get("title"))
                    break
        risks.append(
            DependencyRisk(
                package=_coerce_str(name),
                installed_version=version,
                severity=severity,
                advisory_id=advisory_id,
                summary=summary,
            )
        )
    return risks


def _parse_osv_scanner(data: object) -> list[DependencyRisk]:
    """osv-scanner ``--format json`` -> results[].packages[].vulnerabilities[]."""
    if not isinstance(data, dict):
        return []
    results = data.get("results")
    if not isinstance(results, list):
        return []
    risks: list[DependencyRisk] = []
    for result in results:
        if not isinstance(result, dict):
            continue
        packages = result.get("packages") or []
        if not isinstance(packages, list):
            continue
        for package in packages:
            if not isinstance(package, dict):
                continue
            pkg_info = package.get("package") or {}
            name = _coerce_str(pkg_info.get("name")) if isinstance(pkg_info, dict) else ""
            version = _coerce_str(pkg_info.get("version")) if isinstance(pkg_info, dict) else ""
            vulns = package.get("vulnerabilities") or []
            if not isinstance(vulns, list):
                continue
            for vuln in vulns:
                if not isinstance(vuln, dict):
                    continue
                risks.append(
                    DependencyRisk(
                        package=name,
                        installed_version=version,
                        severity=_osv_severity(vuln),
                        advisory_id=_coerce_str(vuln.get("id"), "UNKNOWN") or "UNKNOWN",
                        summary=_coerce_str(vuln.get("summary") or vuln.get("details")),
                    )
                )
    return risks


def _osv_severity(vuln: dict) -> str:
    severity = vuln.get("severity")
    if isinstance(severity, list) and severity:
        first = severity[0]
        if isinstance(first, dict):
            return _coerce_str(first.get("type") or first.get("score"), "unknown") or "unknown"
    if isinstance(severity, str) and severity:
        return severity
    return "unknown"


def scan_dependency_risks(
    project_root: Path,
    *,
    auditor_runner: AuditorRunner | None = None,
    timeout: int = DEFAULT_TIMEOUT,
) -> list[dict[str, str]]:
    """Convenience entry point: returns dependency risks as plain dicts. Never raises."""
    try:
        scanner = ScaScanner(project_root, auditor_runner=auditor_runner, timeout=timeout)
        return [risk.as_dict() for risk in scanner.scan()]
    except Exception:
        return []
