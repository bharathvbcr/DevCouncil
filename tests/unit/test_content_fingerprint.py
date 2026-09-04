"""``content_fingerprint`` must answer "did these bytes change", not "did the
filesystem touch this inode".

Until 2026-09-04 it hashed ``(path, size, mtime_ns)`` while its docstring
claimed it detected content edits. That was wrong in both directions, and both
are pinned here:

* a byte-identical rewrite (a formatter, a branch checkout, a hook rewriting a
  generated file) moved mtime and marked a *current* map stale. On a repo whose
  files are rewritten by build workers this fired constantly, so ``repo map
  stale`` was on permanently and stopped carrying information;
* an edit that preserved size and mtime marked a *changed* tree fresh — the
  fail-open direction, which is the one that can hand an agent a wrong map.
"""

from __future__ import annotations

import os
from pathlib import Path

from devcouncil.indexing.graph.build import content_fingerprint

# The two behavioural tests below deliberately import nothing that only exists
# after the fix, so they fail as assertions against the old size+mtime scheme
# rather than erroring at collection.


def _tree(root: Path) -> list[str]:
    (root / ".devcouncil").mkdir(exist_ok=True)
    for i in range(3):
        (root / f"f{i}.py").write_text(f"print({i})\n", encoding="utf-8")
    return [f"f{i}.py" for i in range(3)]


def test_byte_identical_rewrite_does_not_mark_the_map_stale(tmp_path):
    """The false-stale half. Rewriting identical bytes moves mtime; the old
    scheme called that a content change and cried wolf every session."""
    files = _tree(tmp_path)
    before = content_fingerprint(tmp_path, files)

    target = tmp_path / "f1.py"
    data = target.read_bytes()
    os.utime(target, None)
    target.write_bytes(data)

    assert content_fingerprint(tmp_path, files) == before, (
        "identical bytes must fingerprint identically no matter how many times "
        "they are rewritten"
    )


def test_edit_that_preserves_size_and_mtime_is_still_detected(tmp_path):
    """The fail-open half, and the one that matters. ``cp -p``, ``rsync
    --times`` and tar extraction all restore mtime; ctime is what cannot be
    back-dated, so the cache key must carry it."""
    files = _tree(tmp_path)
    before = content_fingerprint(tmp_path, files)

    target = tmp_path / "f1.py"
    stat = target.stat()
    target.write_bytes(b"print(9)\n")  # same length as print(1)
    os.utime(target, ns=(stat.st_atime_ns, stat.st_mtime_ns))
    assert target.stat().st_mtime_ns == stat.st_mtime_ns, "mtime must be restored"
    assert target.stat().st_size == stat.st_size, "size must be unchanged"

    assert content_fingerprint(tmp_path, files) != before, (
        "a real content change must be detected even when size and mtime are "
        "indistinguishable from the content it replaced"
    )


def test_fingerprint_is_content_addressed_not_history_addressed(tmp_path):
    """Restoring the original bytes must restore the original fingerprint; a
    scheme that only ratchets forward would keep a reverted tree stale."""
    files = _tree(tmp_path)
    before = content_fingerprint(tmp_path, files)

    target = tmp_path / "f1.py"
    target.write_text("print(999)\n", encoding="utf-8")
    assert content_fingerprint(tmp_path, files) != before

    target.write_text("print(1)\n", encoding="utf-8")
    assert content_fingerprint(tmp_path, files) == before


def test_cache_is_advisory_and_never_changes_the_answer(tmp_path):
    """Losing or corrupting the digest cache may cost time; it must never move
    the fingerprint, or two machines would disagree about one tree."""
    from devcouncil.indexing.graph.build import _content_cache_path

    files = _tree(tmp_path)
    warm = content_fingerprint(tmp_path, files)
    cache = _content_cache_path(tmp_path)
    assert cache.is_file(), "the cache should have been written"

    cache.unlink()
    assert content_fingerprint(tmp_path, files) == warm, "cold rebuild must agree"

    cache.write_text("{ not json", encoding="utf-8")
    assert content_fingerprint(tmp_path, files) == warm, "corrupt cache must agree"

    cache.write_text('{"scheme": "c0", "entries": {"f1.py": ["x", "deadbeef"]}}', encoding="utf-8")
    assert content_fingerprint(tmp_path, files) == warm, "foreign scheme must be ignored"


def test_cache_lives_where_the_inventory_cannot_see_it(tmp_path):
    """The cache is written on every call. If it were inventoried it would
    invalidate the very fingerprint it exists to compute — stale forever."""
    from devcouncil.indexing.graph.build import _content_cache_path
    from devcouncil.indexing.repo_mapper import RepoMapper

    files = _tree(tmp_path)
    content_fingerprint(tmp_path, files)
    cache = _content_cache_path(tmp_path)
    assert ".devcouncil" in cache.parts

    mapper = RepoMapper(tmp_path)
    rel = str(cache.relative_to(tmp_path))
    assert mapper._is_runtime_or_generated_file(rel), (
        "the digest cache must be excluded from the fingerprinted inventory"
    )


def test_scheme_prefix_makes_an_upgrade_read_stale_exactly_once(tmp_path):
    """A map stamped by the old size+mtime scheme is a bare sha1. It must not
    accidentally compare equal to a content digest — fail closed, rebuild."""
    from devcouncil.indexing.graph.build import _CONTENT_SCHEME

    files = _tree(tmp_path)
    current = content_fingerprint(tmp_path, files)
    assert current.startswith(f"{_CONTENT_SCHEME}:"), "scheme must be declared inline"
    assert len(current.split(":", 1)[1]) == 40, "body stays a sha1 hexdigest"
