"""Content-hash keyed cache of green gate results for incremental verification.

Persisted at ``.devcouncil/cache/gate_results.json``. A gate is cached green only when
it *passed*; the cache key is a hash over the command string plus the byte content of
the gate's declared inputs (see :class:`~devcouncil.verification.gate_selector.GateSpec`).
On the next run, a gate whose input hash matches a stored green entry is skipped — so an
edit to file A never re-runs the gate that only depends on file B.

Failing results are recorded too (so the sidecar can show a cached FAIL without a
re-run), but ``is_green`` is the only thing that authorizes a *skip*; a cached failure
is always re-run so a fix is picked up immediately.

Best-effort and crash-safe: a corrupt/absent cache degrades to "nothing cached" (every
gate runs), and writes go through the atomic JSON helper. This cache is an optimization,
never a source of truth — the full verify path does not consult it.
"""

from __future__ import annotations

import hashlib
import logging
import time
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING

from devcouncil.utils.json_persist import read_json, write_json

if TYPE_CHECKING:
    from devcouncil.verification.gate_selector import GateSpec

logger = logging.getLogger(__name__)

_CACHE_VERSION = 1
_ABSENT = "\0absent\0"

_CONFIG_FILES = (
    "pyproject.toml",
    "ruff.toml",
    ".ruff.toml",
    "mypy.ini",
    "pyrightconfig.json",
    "tsconfig.json",
    "package-lock.json",
    "uv.lock",
    "poetry.lock",
)


@dataclass
class _Entry:
    input_hash: str
    passed: bool
    summary: str
    updated_at: float


class GateResultCache:
    """Load/query/update the on-disk green-gate cache.

    Instantiate once per verify pass; call :meth:`is_green` to decide skips, then
    :meth:`record` after running a gate, and :meth:`save` once at the end.
    """

    def __init__(self, project_root: Path):
        self.project_root = Path(project_root)
        self._entries: dict[str, _Entry] = {}
        self._loaded = False

    @property
    def path(self) -> Path:
        return self.project_root / ".devcouncil" / "cache" / "gate_results.json"

    def load(self) -> "GateResultCache":
        self._loaded = True
        self._entries = {}
        if not self.path.exists():
            return self
        try:
            data = read_json(self.path)
        except Exception as exc:
            logger.debug("gate cache unreadable (%s); treating as empty", exc)
            return self
        if not isinstance(data, dict) or data.get("version") != _CACHE_VERSION:
            return self
        gates = data.get("gates")
        if not isinstance(gates, dict):
            return self
        for name, raw in gates.items():
            if not isinstance(raw, dict) or "input_hash" not in raw:
                continue
            self._entries[name] = _Entry(
                input_hash=str(raw.get("input_hash", "")),
                passed=bool(raw.get("passed", False)),
                summary=str(raw.get("summary", "")),
                updated_at=float(raw.get("updated_at", 0.0) or 0.0),
            )
        return self

    def _ensure_loaded(self) -> None:
        if not self._loaded:
            self.load()

    def _file_hash(self, rel_path: str) -> str:
        """SHA-256 of a file's bytes, or a sentinel for a missing/unreadable file.

        Missing is a distinct value (not skipped) so that deleting or creating an input
        changes the hash and correctly invalidates a previously-green gate."""
        fp = self.project_root / rel_path
        try:
            if not fp.is_file():
                return _ABSENT
            digest = hashlib.sha256()
            with fp.open("rb") as handle:
                for chunk in iter(lambda: handle.read(65536), b""):
                    digest.update(chunk)
            return digest.hexdigest()
        except OSError:
            return _ABSENT

    def _config_fingerprint(self) -> str:
        """Hash mtime+size of known project config files that affect gate behavior."""
        digest = hashlib.sha256()
        for name in _CONFIG_FILES:
            fp = self.project_root / name
            digest.update(name.encode("utf-8"))
            digest.update(b"=")
            try:
                if not fp.is_file():
                    digest.update(_ABSENT.encode("utf-8"))
                else:
                    stat = fp.stat()
                    digest.update(f"{stat.st_mtime_ns}:{stat.st_size}".encode("utf-8"))
            except OSError:
                digest.update(_ABSENT.encode("utf-8"))
            digest.update(b"\0")
        return digest.hexdigest()

    def input_hash(self, gate: "GateSpec") -> str:
        """Stable hash over the gate command, config fingerprint, and input file contents."""
        digest = hashlib.sha256()
        digest.update(gate.command.encode("utf-8"))
        digest.update(b"\0")
        digest.update(self._config_fingerprint().encode("utf-8"))
        digest.update(b"\0")
        for rel_path in sorted(gate.inputs):
            digest.update(rel_path.encode("utf-8"))
            digest.update(b"=")
            digest.update(self._file_hash(rel_path).encode("utf-8"))
            digest.update(b"\0")
        return digest.hexdigest()

    def is_green(self, gate: "GateSpec") -> bool:
        """True when this gate previously passed with byte-identical inputs and command."""
        self._ensure_loaded()
        entry = self._entries.get(gate.name)
        if entry is None or not entry.passed:
            return False
        return entry.input_hash == self.input_hash(gate)

    def cached_summary(self, gate: "GateSpec") -> str | None:
        self._ensure_loaded()
        entry = self._entries.get(gate.name)
        return entry.summary if entry is not None else None

    def record(self, gate: "GateSpec", *, passed: bool, summary: str = "") -> None:
        """Update the in-memory entry for ``gate`` (call :meth:`save` to persist)."""
        self._ensure_loaded()
        self._entries[gate.name] = _Entry(
            input_hash=self.input_hash(gate),
            passed=bool(passed),
            summary=summary,
            updated_at=time.time(),
        )

    def save(self) -> None:
        """Atomically persist the cache. Best-effort: a write failure is logged, not raised."""
        payload = {
            "version": _CACHE_VERSION,
            "gates": {
                name: {
                    "input_hash": entry.input_hash,
                    "passed": entry.passed,
                    "summary": entry.summary,
                    "updated_at": entry.updated_at,
                }
                for name, entry in self._entries.items()
            },
        }
        try:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            write_json(self.path, payload, indent=2, sort_keys=True)
        except Exception as exc:
            logger.debug("failed to persist gate cache: %s", exc)
