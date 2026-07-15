"""One index/watch scope policy backed by Git ignore semantics."""

from __future__ import annotations

import subprocess
from pathlib import Path

from devcouncil.codeintel.languages import detect_language

_IGNORED_PREFIXES = (
    ".git/",
    ".devcouncil/",
    "node_modules/",
    "vendor/",
    ".venv/",
    "venv/",
    "dist/",
    "build/",
    "target/",
    "__pycache__/",
    ".pytest_cache/",
    ".mypy_cache/",
    ".ruff_cache/",
)


class IndexScope:
    def __init__(self, root: Path):
        self.root = root.expanduser().resolve()

    def includes(self, path: str | Path) -> bool:
        try:
            rel = self.relative(path)
        except ValueError:
            return False
        if any(rel == prefix.rstrip("/") or rel.startswith(prefix) for prefix in _IGNORED_PREFIXES):
            return False
        if detect_language(rel) is None:
            return False
        return not self._git_ignored(rel)

    def relative(self, path: str | Path) -> str:
        candidate = Path(path)
        if not candidate.is_absolute():
            candidate = self.root / candidate
        return candidate.resolve(strict=False).relative_to(self.root).as_posix()

    def files(self) -> list[str]:
        try:
            proc = subprocess.run(
                ["git", "ls-files", "-co", "--exclude-standard", "-z"],
                cwd=self.root,
                check=False,
                capture_output=True,
                timeout=10,
            )
            if proc.returncode == 0:
                values = proc.stdout.decode("utf-8", errors="replace").split("\0")
                return sorted({value for value in values if value and self.includes(value)})
        except (OSError, subprocess.SubprocessError):
            pass
        return sorted(
            path.relative_to(self.root).as_posix()
            for path in self.root.rglob("*")
            if path.is_file() and self.includes(path)
        )

    def _git_ignored(self, rel: str) -> bool:
        # Fast exclusions cover high-volume directories. Git is the authority for
        # project-specific rules and correctly handles nested .gitignore files.
        try:
            result = subprocess.run(
                ["git", "check-ignore", "-q", "--", rel],
                cwd=self.root,
                check=False,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=2,
            )
            return result.returncode == 0
        except (OSError, subprocess.SubprocessError):
            return False
