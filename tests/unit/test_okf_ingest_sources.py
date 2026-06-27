"""`fetch_bundle` materializes local dirs, archives, and (mocked) git URLs for ingest.

These tests stay fully offline: archives are built in ``tmp_path`` and the git path is
exercised only with git monkeypatched as missing. The traversal guard must reject any
archive entry that escapes the extraction directory (a path-traversal / Zip-Slip block).
"""

import tarfile
import zipfile

import pytest

from devcouncil.knowledge.fetch import UnsafeArchiveError, fetch_bundle
from devcouncil.knowledge.okf import OKFBundle, OKFDocument, read_bundle, write_bundle


def _make_bundle(directory):
    """Write a tiny, link-clean OKF bundle into ``directory``."""
    bundle = OKFBundle(documents=[
        OKFDocument(type="OKF Index", title="Root", body="root index", rel_path="index.md"),
        OKFDocument(type="Note", title="Alpha", body="hello alpha", rel_path="notes/alpha.md"),
    ])
    write_bundle(bundle, directory)
    return directory


def _targz(src_dir, archive):
    with tarfile.open(archive, "w:gz") as tf:
        for path in sorted(src_dir.rglob("*")):
            if path.is_file():
                tf.add(path, arcname=str(path.relative_to(src_dir)))


def _zip(src_dir, archive):
    with zipfile.ZipFile(archive, "w") as zf:
        for path in sorted(src_dir.rglob("*")):
            if path.is_file():
                zf.write(path, arcname=str(path.relative_to(src_dir)))


def test_fetch_local_directory_returns_as_is(tmp_path):
    src = _make_bundle(tmp_path / "bundle")
    fetched = fetch_bundle(str(src))
    try:
        assert fetched.cleanup_dir is None  # nothing to remove for a real directory
        assert fetched.directory == src.resolve()
        by_path = read_bundle(fetched.directory).by_path()
        assert "index.md" in by_path
        assert "notes/alpha.md" in by_path
    finally:
        fetched.cleanup()


def test_fetch_targz_extracts_to_readable_bundle(tmp_path):
    src = _make_bundle(tmp_path / "bundle")
    archive = tmp_path / "bundle.tar.gz"
    _targz(src, archive)

    fetched = fetch_bundle(str(archive))
    try:
        assert fetched.cleanup_dir is not None
        assert fetched.suggested_name == "bundle"
        by_path = read_bundle(fetched.directory).by_path()
        assert "index.md" in by_path
        assert "notes/alpha.md" in by_path
    finally:
        fetched.cleanup()
    assert not fetched.cleanup_dir.exists()  # cleanup removed the temp dir


def test_fetch_zip_extracts_to_readable_bundle(tmp_path):
    src = _make_bundle(tmp_path / "bundle")
    archive = tmp_path / "bundle.zip"
    _zip(src, archive)

    fetched = fetch_bundle(str(archive))
    try:
        by_path = read_bundle(fetched.directory).by_path()
        assert "index.md" in by_path
        assert "notes/alpha.md" in by_path
    finally:
        fetched.cleanup()


def test_targz_traversal_entry_is_rejected(tmp_path):
    payload = tmp_path / "payload.md"
    payload.write_text("evil", encoding="utf-8")
    archive = tmp_path / "evil.tar.gz"
    with tarfile.open(archive, "w:gz") as tf:
        tf.add(payload, arcname="../evil.md")  # escapes the extraction dir

    with pytest.raises(UnsafeArchiveError):
        fetch_bundle(str(archive))
    assert not (tmp_path / "evil.md").exists()  # nothing was written outside the temp dir


def test_zip_traversal_entry_is_rejected(tmp_path):
    archive = tmp_path / "evil.zip"
    with zipfile.ZipFile(archive, "w") as zf:
        zf.writestr("../evil.md", "evil")

    with pytest.raises(ValueError):  # UnsafeArchiveError subclasses ValueError
        fetch_bundle(str(archive))
    assert not (tmp_path / "evil.md").exists()


def test_missing_source_raises(tmp_path):
    with pytest.raises(FileNotFoundError):
        fetch_bundle(str(tmp_path / "does-not-exist"))


def test_git_url_without_git_raises_clear_error(monkeypatch):
    # Offline: pretend git is not installed and assert a clear error for a git-URL source.
    import devcouncil.knowledge.fetch as fetch_mod

    monkeypatch.setattr(fetch_mod.shutil, "which", lambda _name: None)
    with pytest.raises(RuntimeError, match="git is not installed"):
        fetch_bundle("https://example.com/some/bundle.git")
