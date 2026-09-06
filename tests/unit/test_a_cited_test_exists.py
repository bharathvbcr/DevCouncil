"""A comment that names a test by path must name a test that exists.

``DEVMAP_REVIEW`` R11 found a doc comment claiming a rule was "pinned by
``language_import_capabilities.rs``" — a file that has never existed in this
repository. Its own point was that this is the failure it condemned
``CALL_EXTRACTION_LANGUAGES`` for, reintroduced in the documentation of the
replacement. Sweeping for the shape found **five** more, in five different
places, two of which are worse than a wrong name:

* ``devmap_client._norm_repo_path`` said it was "frozen by
  ``test_symbol_is_reached.py``, which asserts the two agree". Nothing anywhere
  referenced ``_norm_repo_path`` outside its own module. The drift check was
  documented and not written.
* ``test_cost_command.py`` pointed at a JSON-stdout contract test under a
  tests/e2e directory, said to re-prove the contract "through a real
  subprocess". No such directory has ever existed. The stronger half of the
  contract was documented and not written.
* ``repo_mapper._CacheDirectoryCache`` cited freshness_parity.rs by a bare
  tests/ path — a real file at ``rust-port/crates/devmap-query/tests/`` and at
  no path that resolves from a Python module.
* ``query_phase_ab.rs`` cited a traversal-allocation test under tests/ for a
  property that ``engine.rs`` pins inline, in a file of that name that exists
  nowhere.
* ``pyproject.toml`` named two tests as holding the suite-collection invariant.
  One of the two has never existed; the real second half is a function inside
  the first.

A citation is a claim, and this is the one class of claim a machine can settle:
either the path names a file or it does not. A reader who follows a provenance
citation and finds nothing learns that the claim is unverifiable; a reader who
does not follow it carries a false belief that something is checked.

**The convention this creates.** A ``tests/…`` path inside backticks is a live
pointer and must resolve. Prose about a citation that was wrong — "it used to
name a test under tests/e2e" — writes the path *without* backticks, because a
dead path in backticks is indistinguishable from a live one that broke, and a
guard that cannot tell them apart must refuse both rather than guess.

**Scope, stated rather than implied.** Only *paths* are checked, and only in
sources and build configuration — ``.py``, ``.rs``, ``.toml``, ``.yml`` — never
Markdown. Configuration is in scope because that is where the sixth instance
lived: ``pyproject.toml``'s ``pythonpath`` setting carries the longest
provenance comment in the repository, and one of the two tests it named as
holding the invariant did not exist. It lives on the Python side rather than in
``cargo test`` because the defect crosses languages in one direction:
``repo_mapper.py`` cited a Rust test, and a guard inside the Rust workspace
could not have seen it. This one sees both trees, so there is one owner rather
than two that each cover part of the surface. Two deliberate exclusions:

* A **bare basename** — a name in backticks with no directory — is not checked.
  Prose legitimately names files that are not in this tree: ``foo.rs`` and
  ``a/b.rs`` as grammar examples, ``src/bin/tool.rs`` as a fixture inside a test
  corpus. Sweeping the Rust tree found twelve such mentions, ten of them
  legitimate, so a basename rule would run five parts noise to one part signal
  and would train writers to add exclusions rather than fix citations. Cite a
  test by its path and it is checked; the cost of being checkable is one
  directory prefix.
* **Markdown** — ``STATUS.md``, the audit reports and ``IMPROVEMENTS.md`` cite
  crate-relative paths under tests/ with no crate anchor, and they are
  historical records of what was true when written rather than claims about
  the tree as it stands.
"""

from __future__ import annotations

import os
import re
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]

#: Trees that are not this repository's source: dependencies, build output, and
#: the sibling checkouts other agents work in. A worktree is a *copy*, so every
#: finding in one is a duplicate of a finding here, and a stale copy would fail
#: this test for a defect that no longer exists in the tree under test.
#:
#: Names alone are not enough. This checkout carries nine cargo target
#: directories — ``target``, ``target-split``, ``target-audit``, ``target-laneD``
#: and five more, one per build lane — holding 747,390 files between them, 76.8%
#: of everything under the root. Pruning by the literal name ``target`` leaves
#: eight of the nine in, and a name list has to grow every time a lane is added.
#: See :func:`_sources` for the rule that actually settles it: all nine carry a
#: ``CACHEDIR.TAG``, because cargo writes one.
_SKIP_DIRS = frozenset(
    {
        ".git",
        ".venv",
        "venv",
        "node_modules",
        "target",
        "__pycache__",
        "build",
        "dist",
        ".mypy_cache",
        ".pytest_cache",
        ".ruff_cache",
        "worktrees",
        # Runtime state, not source: agent signals and task checkpoints are
        # gitignored, are written by a running `dev`, and quote paths that were
        # true for one task on one machine.
        ".devcouncil",
    }
)

#: The Cache Directory Tagging Standard marker. A directory holding a file of
#: this name whose content begins with the signature below is a cache, whoever
#: wrote it — cargo, pip, uv, ccache, tox, ruff and pytest all do. This repo
#: already prunes discovery on exactly this rule in two places
#: (``devmap_extract::CacheDirectoryCache`` and
#: ``repo_mapper._CacheDirectoryCache``); this is the third reader, not a fourth
#: convention. Checked by signature rather than by filename so a source file
#: that happens to be called ``CACHEDIR.TAG`` cannot hide a subtree.
_CACHEDIR_TAG_FILE = "CACHEDIR.TAG"
_CACHEDIR_TAG_SIGNATURE = b"Signature: 8a477f597d28d172789f06886806bc55"

#: Sources and build configuration. Markdown is excluded on purpose — see the
#: module docstring.
_SOURCE_SUFFIXES = frozenset({".py", ".rs", ".toml", ".yml", ".yaml"})

#: A path citation, in single or double backticks, rooted at a ``tests``
#: directory. Anchored at ``tests/`` because that prefix is what makes the
#: citation resolvable at all — see the module docstring on bare basenames.
_CITATION = re.compile(
    r"`{1,2}("
    r"(?:rust-port/crates/[A-Za-z0-9_-]+/)?tests/[A-Za-z0-9_./-]+\.(?:rs|py)"
    r")`{1,2}"
)

#: The tree held 29 resolvable test-path citations across 1,129 sources when
#: this was written. A regex that stopped matching, or a walk that pruned the
#: wrong subtree, would make this test pass by finding nothing — the failure
#: mode it exists to prevent — so a floor is asserted. Set well below the count
#: so ordinary edits never trip it, and well above zero so a pattern that has
#: gone dead does. Lower it only alongside a deletion that explains where the
#: citations went.
_MINIMUM_CITATIONS = 15


def _is_cache_dir(directory: str, names: list[str]) -> bool:
    """True for a directory tagged under the Cache Directory Tagging Standard."""
    if _CACHEDIR_TAG_FILE not in names:
        return False
    try:
        with open(os.path.join(directory, _CACHEDIR_TAG_FILE), "rb") as handle:
            return handle.read(len(_CACHEDIR_TAG_SIGNATURE)) == _CACHEDIR_TAG_SIGNATURE
    except OSError:
        return False


def _sources() -> list[Path]:
    """Every source and configuration file in this checkout.

    ``os.walk`` rather than ``Path.iterdir``: the latter costs two extra stats
    per entry (``is_symlink``, ``is_dir``), which over this checkout was 7.0s
    against 0.6s for the same result. Build output is pruned by its
    ``CACHEDIR.TAG`` rather than by name — the rule that scales, since every
    lane's target directory carries one and no name list can keep up with lanes
    being added.
    """
    found: list[Path] = []
    for directory, subdirs, files in os.walk(os.fspath(REPO_ROOT), followlinks=False):
        if _is_cache_dir(directory, files):
            subdirs[:] = []
            continue
        subdirs[:] = [name for name in subdirs if name not in _SKIP_DIRS]
        for name in files:
            if Path(name).suffix in _SOURCE_SUFFIXES:
                found.append(Path(directory) / name)
    return found


def _resolutions(source: Path, cited: str) -> list[Path]:
    """Every root the citation is allowed to be relative to.

    The repo root always, because that is what ``tests/`` means to anything
    outside the Rust workspace. A ``.rs`` file may also mean its own crate: a
    module under ``crates/devmap-query/src/`` citing a bare tests/ path means
    ``crates/devmap-query/tests/``, which is how Cargo lays a crate out and how
    every correct citation in the workspace is written today.
    """
    roots = [REPO_ROOT]
    if source.suffix == ".rs":
        parts = source.relative_to(REPO_ROOT).parts
        if "crates" in parts:
            index = parts.index("crates")
            roots.append(REPO_ROOT.joinpath(*parts[: index + 2]))
    return [root / cited for root in roots]


def test_every_cited_test_path_names_a_file_that_exists() -> None:
    unresolved: list[str] = []
    total = 0

    for source in _sources():
        try:
            text = source.read_text(encoding="utf-8")
        except (UnicodeDecodeError, OSError):
            continue
        if "tests/" not in text:
            continue
        for lineno, line in enumerate(text.splitlines(), 1):
            for match in _CITATION.finditer(line):
                cited = match.group(1)
                total += 1
                if any(path.is_file() for path in _resolutions(source, cited)):
                    continue
                where = source.relative_to(REPO_ROOT)
                unresolved.append(f"{where}:{lineno} cites `{cited}` — no such file")

    assert total >= _MINIMUM_CITATIONS, (
        f"only {total} test-path citations found, expected at least "
        f"{_MINIMUM_CITATIONS}; the pattern has stopped matching and this test "
        "is now vacuous"
    )
    assert not unresolved, (
        "a comment names a test that does not exist, so the claim it makes "
        "cannot be checked by anyone who follows it:\n  "
        + "\n  ".join(sorted(unresolved))
    )
