"""Materialize an OKF bundle *source* into a local directory for ingest.

``dev okf ingest`` historically accepted only a local bundle directory. Bundles travel,
though — as ``.tar.gz``/``.zip`` archives or behind a git URL — so this module resolves
any of those forms to a concrete on-disk directory the existing read/validate/copy logic
can consume unchanged:

* an existing local directory → returned as-is (no temp dir, nothing to clean up);
* a local archive (``.tar.gz``/``.tgz``/``.zip``) → extracted into a temp dir, with a
  **path-traversal guard** that rejects entries (or link targets) escaping the target;
* a git URL (``http(s)://``, ``git@``, ``ssh://``, or ``*.git``) → ``git clone --depth 1``
  into a temp dir (best-effort; a clear error is raised if git is missing or the clone
  fails).

Callers are responsible for invoking :meth:`FetchedBundle.cleanup` (in a ``finally``) to
remove any temp dir once the bundle has been read/copied.
"""

from __future__ import annotations

import shutil
import subprocess
import tarfile
import tempfile
import zipfile
from dataclasses import dataclass
from pathlib import Path

_ARCHIVE_SUFFIXES = (".tar.gz", ".tgz", ".zip")


class UnsafeArchiveError(ValueError):
    """An archive entry (or link target) resolves outside the extraction directory.

    Subclasses :class:`ValueError` so callers can catch either; raised by the extraction
    guard before any unsafe member is written to disk (a path-traversal / Zip-Slip block).
    """


@dataclass
class FetchedBundle:
    """The resolved local bundle directory plus any temp dir that must be cleaned up.

    ``directory`` is the bundle root to read. ``cleanup_dir`` is the temp directory created
    for archives/git (``None`` for a pre-existing local directory, which is returned as-is
    and must NOT be deleted). ``suggested_name`` is a sensible default ingest subfolder name
    derived from the original source (its temp dir name would otherwise be random).
    """

    directory: Path
    cleanup_dir: Path | None
    suggested_name: str = ""

    def cleanup(self) -> None:
        """Remove the temp dir if one was created; safe to call when there is none."""
        if self.cleanup_dir is not None:
            shutil.rmtree(self.cleanup_dir, ignore_errors=True)


def is_git_url(source: str) -> bool:
    """Whether ``source`` looks like a git remote we should ``git clone``."""
    s = source.strip()
    return (
        s.startswith("http://")
        or s.startswith("https://")
        or s.startswith("git@")
        or s.startswith("ssh://")
        or s.endswith(".git")
    )


def _archive_stem(name: str) -> str:
    """The base name of an archive file with its (possibly two-part) suffix removed."""
    lower = name.lower()
    for suffix in _ARCHIVE_SUFFIXES:
        if lower.endswith(suffix):
            return name[: -len(suffix)] or "bundle"
    return Path(name).stem or "bundle"


def _git_repo_name(url: str) -> str:
    tail = url.rstrip("/").split("/")[-1].split(":")[-1]
    if tail.endswith(".git"):
        tail = tail[:-4]
    return tail or "bundle"


def _within(base: Path, target: Path) -> bool:
    """Whether resolved ``target`` is ``base`` itself or lives beneath it."""
    try:
        target.relative_to(base)
        return True
    except ValueError:
        # relative_to only raises when target is neither base nor beneath it, so an
        # escaping path is unambiguously outside the extraction root.
        return False


def _safe_extract_tar(archive: Path, dest: Path) -> None:
    """Extract a tar archive into ``dest``, rejecting any path-escaping member/link."""
    dest_resolved = dest.resolve()
    with tarfile.open(archive, "r:*") as tf:
        members = tf.getmembers()
        for member in members:
            target = (dest / member.name).resolve()
            if not _within(dest_resolved, target):
                raise UnsafeArchiveError(
                    f"unsafe archive entry {member.name!r} escapes the extraction directory"
                )
            # A symlink/hardlink could still point outside even if its own path is safe.
            # Symlink ``linkname`` is relative to the link's own directory; hardlink
            # ``linkname`` is relative to the archive root — resolve each against the
            # correct base, else a hardlink escaping via the root is mis-validated.
            if member.issym() or member.islnk():
                base = target.parent if member.issym() else dest
                link_target = (base / member.linkname).resolve()
                if not _within(dest_resolved, link_target):
                    raise UnsafeArchiveError(
                        f"unsafe link target {member.linkname!r} in entry {member.name!r}"
                    )
        # ``filter="data"`` is the safe extraction default on Python 3.12+ (defense in depth
        # alongside the explicit guard above); fall back gracefully on older interpreters.
        try:
            tf.extractall(dest, members=members, filter="data")
        except TypeError:  # pragma: no cover - Python < 3.12 has no filter kwarg
            tf.extractall(dest, members=members)


def _safe_extract_zip(archive: Path, dest: Path) -> None:
    """Extract a zip archive into ``dest``, rejecting any path-escaping member (Zip-Slip)."""
    dest_resolved = dest.resolve()
    with zipfile.ZipFile(archive) as zf:
        for name in zf.namelist():
            target = (dest / name).resolve()
            if not _within(dest_resolved, target):
                raise UnsafeArchiveError(
                    f"unsafe archive entry {name!r} escapes the extraction directory"
                )
        zf.extractall(dest)


def _resolve_bundle_root(extracted: Path) -> Path:
    """Descend into a lone top-level directory.

    Archives produced with ``tar czf x.tgz somedir`` (or a git repo whose bundle lives in
    a subdir) nest everything under one directory; collapsing it makes the returned path the
    actual bundle root. If the markdown already sits at the top level, the dir is used as-is.
    Purely best-effort: :func:`read_bundle` recurses anyway, so a wrong guess is harmless.
    """
    entries = [p for p in extracted.iterdir() if not p.name.startswith(".")]
    if any(p.is_file() and p.suffix == ".md" for p in entries):
        return extracted
    if len(entries) == 1 and entries[0].is_dir():
        return entries[0]
    return extracted


def fetch_bundle(source: str) -> FetchedBundle:
    """Resolve ``source`` (local dir, local archive, or git URL) to a local bundle dir.

    Raises :class:`UnsafeArchiveError` for a path-escaping archive entry, :class:`FileNotFoundError`
    for a non-existent local path that isn't a git URL, and :class:`RuntimeError` if a git
    clone is required but git is missing or the clone fails.
    """
    raw = source.strip()
    local = Path(raw).expanduser()

    # (a) existing local directory — use it directly; nothing to clean up.
    if local.is_dir():
        resolved = local.resolve()
        return FetchedBundle(directory=resolved, cleanup_dir=None, suggested_name=resolved.name)

    # (b) local archive — extract into a temp dir behind the traversal guard.
    if local.is_file() and local.name.lower().endswith(_ARCHIVE_SUFFIXES):
        tmp = Path(tempfile.mkdtemp(prefix="okf-archive-"))
        try:
            if local.name.lower().endswith(".zip"):
                _safe_extract_zip(local, tmp)
            else:
                _safe_extract_tar(local, tmp)
        except BaseException:
            shutil.rmtree(tmp, ignore_errors=True)
            raise
        return FetchedBundle(
            directory=_resolve_bundle_root(tmp),
            cleanup_dir=tmp,
            suggested_name=_archive_stem(local.name),
        )

    # (c) git URL — shallow clone into a temp dir.
    if is_git_url(raw):
        return _clone_git(raw)

    raise FileNotFoundError(
        f"bundle source not found: {source!r} (expected a directory, a .tar.gz/.tgz/.zip "
        "archive, or a git URL)"
    )


def _clone_git(url: str) -> FetchedBundle:
    """``git clone --depth 1`` ``url`` into a fresh temp dir (best-effort)."""
    if shutil.which("git") is None:
        raise RuntimeError("git is not installed; cannot clone bundle from a git URL")
    parent = Path(tempfile.mkdtemp(prefix="okf-git-"))
    target = parent / "clone"
    try:
        result = subprocess.run(
            ["git", "clone", "--depth", "1", url, str(target)],
            capture_output=True,
            text=True,
        )
    except OSError as exc:  # pragma: no cover - git present but unexecutable
        shutil.rmtree(parent, ignore_errors=True)
        raise RuntimeError(f"git clone failed to start: {exc}") from exc
    if result.returncode != 0:
        shutil.rmtree(parent, ignore_errors=True)
        detail = (result.stderr or result.stdout or "").strip()
        raise RuntimeError(f"git clone of {url!r} failed: {detail}")
    return FetchedBundle(
        directory=_resolve_bundle_root(target),
        cleanup_dir=parent,
        suggested_name=_git_repo_name(url),
    )
