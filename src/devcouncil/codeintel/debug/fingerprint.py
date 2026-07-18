"""Source, build, and executable fingerprints for runtime evidence."""

from __future__ import annotations

import hashlib
import subprocess
from pathlib import Path


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def executable_hash(path: str | Path) -> str:
    try:
        return _sha256(Path(path).resolve().read_bytes())
    except OSError:
        return ""


_GIT_TIMEOUT_SECONDS = 15.0
_FALLBACK_SKIP_PARTS = frozenset({
    ".git", ".devcouncil", "node_modules", "vendor", ".venv", "venv",
    "dist", "build", "target", "__pycache__",
})


def source_fingerprint(root: Path) -> str:
    root = root.expanduser().resolve()
    head = b""
    dirty = b""
    try:
        head = subprocess.check_output(
            ["git", "rev-parse", "HEAD"],
            cwd=root,
            stderr=subprocess.DEVNULL,
            timeout=_GIT_TIMEOUT_SECONDS,
        )
        dirty = subprocess.check_output(
            ["git", "diff", "--no-ext-diff", "--binary", "HEAD"],
            cwd=root,
            stderr=subprocess.DEVNULL,
            timeout=_GIT_TIMEOUT_SECONDS,
        )
        untracked = subprocess.check_output(
            ["git", "ls-files", "--others", "--exclude-standard", "-z"],
            cwd=root,
            stderr=subprocess.DEVNULL,
            timeout=_GIT_TIMEOUT_SECONDS,
        )
        for raw_path in sorted(value for value in untracked.split(b"\0") if value):
            path = root / raw_path.decode("utf-8", errors="surrogateescape")
            try:
                dirty += b"\0untracked:" + raw_path + b"\0" + _sha256(path.read_bytes()).encode()
            except OSError:
                continue
    except (OSError, subprocess.SubprocessError):
        for path in sorted(root.rglob("*")):
            if not path.is_file() or _FALLBACK_SKIP_PARTS.intersection(path.parts):
                continue
            try:
                stat = path.stat()
                dirty += f"{path.relative_to(root).as_posix()}\0{stat.st_size}\0{stat.st_mtime_ns}\n".encode()
            except OSError:
                continue
    return _sha256(head + b"\0" + dirty)


def build_fingerprint(root: Path, executable: str | Path = "") -> str:
    parts = [source_fingerprint(root).encode()]
    for rel in (
        ".devcouncil/config.yaml",
        "pyproject.toml",
        "package.json",
        "Cargo.toml",
        "go.mod",
        "CMakeLists.txt",
    ):
        try:
            parts.append((root / rel).read_bytes())
        except OSError:
            pass
    if executable:
        parts.append(executable_hash(executable).encode())
    return _sha256(b"\0".join(parts))
