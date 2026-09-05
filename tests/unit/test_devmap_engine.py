"""Regression coverage for the Rust map engine seam.

Two defects found while wiring it, both by measurement rather than review:

1. A **relative** output path was resolved against the process's cwd instead of
   the project root. `dev map --project-root /other/repo` passes the default
   `.devcouncil/repo_map.json`, so the engine read and rewrote the map of
   whichever repository the shell happened to be in. It corrupted this
   repository's own map during development.
2. A stale `devmap` on PATH reports the *same version string* as the freshly
   built one and lacks `--graph-output`, so a version check is not evidence of
   a capability.
"""

from __future__ import annotations

import json
import logging
import os
import subprocess
import tempfile
from pathlib import Path

import pytest

from devcouncil import devmap_engine
from devcouncil.devmap_engine import DevMapEngineError, build_map, find_engine_binary


def _git_repo(tmp_path: Path) -> Path:
    root = tmp_path / "repo"
    root.mkdir()
    (root / "k.py").write_text("def helper(a): return a\ndef main(): return helper(1)\n")
    subprocess.run(["git", "init", "-q", "."], cwd=root, check=True)
    subprocess.run(["git", "add", "-A"], cwd=root, check=True)
    subprocess.run(
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "init"],
        cwd=root,
        check=True,
    )
    return root


def _have_engine() -> bool:
    try:
        find_engine_binary()
    except DevMapEngineError:
        return False
    return True


requires_engine = pytest.mark.skipif(
    not _have_engine(), reason="devmap kernel not built (cargo build --release -p devmap-cli)"
)


@requires_engine
def test_a_relative_output_lands_under_the_project_root_not_the_cwd(tmp_path, monkeypatch):
    """The cross-repository write that corrupted this repo's map.

    The assertion that matters is the negative one: a *decoy* map sitting at the
    same relative path under the cwd must be byte-identical afterwards. Checking
    only that the target was written would have passed while the bug was live.
    """
    root = _git_repo(tmp_path)
    elsewhere = tmp_path / "cwd"
    (elsewhere / ".devcouncil").mkdir(parents=True)
    decoy = elsewhere / ".devcouncil" / "repo_map.json"
    decoy.write_text('{"indexed_hash": "DECOY", "files": []}')
    before = decoy.read_text()

    monkeypatch.chdir(elsewhere)
    build_map(root, output=Path(".devcouncil/repo_map.json"))

    assert (root / ".devcouncil" / "repo_map.json").is_file(), "target repo must get the map"
    assert decoy.read_text() == before, "a map outside the project root must never be touched"


@requires_engine
def test_freshness_is_stamped_and_actually_detects_change(tmp_path):
    """A fingerprint that always says 'fresh' is worse than none.

    The Rust kernel writes both digests empty, and `map_is_stale` skips its
    check only when `generated_head` is *also* empty — which it is not. So an
    unstamped map reads permanently stale and `--if-stale` never short-circuits.
    Both directions are asserted; the second is the one that matters.
    """
    from devcouncil.indexing.repo_mapper import RepoMapper

    root = _git_repo(tmp_path)
    written = build_map(root)
    payload = json.loads(written.read_text())

    assert payload["indexed_hash"], "indexed_hash must be stamped"
    assert payload["content_fingerprint"], "content_fingerprint must be stamped"

    mapper = RepoMapper(project_root=root)
    assert mapper.map_is_stale(payload) is False, "a just-built map must read fresh"

    # No sleep: content_fingerprint hashes bytes, not mtime, so a changed file is
    # detected immediately. Under the old scheme this needed 1.1s to outrun stat
    # granularity — which was the tell that it was timing edits, not reading them.
    (root / "k.py").write_text("def helper(a): return a + 1\ndef main(): return helper(1)\n")
    assert mapper.map_is_stale(payload) is True, "an edited file must read stale"


@requires_engine
def test_both_artifacts_come_from_one_invocation(tmp_path):
    """Eleven consumers read code_graph.json; a map without it is a half build."""
    root = _git_repo(tmp_path)
    build_map(root)
    assert (root / ".devcouncil" / "repo_map.json").is_file()
    assert (root / ".devcouncil" / "graph" / "code_graph.json").is_file()


def test_a_binary_without_the_graph_capability_is_refused(tmp_path, monkeypatch):
    """Version is not evidence of capability.

    `~/.cargo/bin/devmap` reports `devmap 0.1.0` — exactly what the freshly
    built kernel reports — and cannot write the graph companion. The probe asks
    what the binary supports rather than what it calls itself.
    """
    fake = tmp_path / "devmap"
    fake.write_text("#!/bin/sh\necho 'Usage: devmap manifest [OPTIONS]'\n")
    fake.chmod(0o755)

    import shutil as _shutil

    # Point the package-relative search at an empty tree so only PATH answers.
    monkeypatch.setattr(devmap_engine, "__file__", str(tmp_path / "a" / "b" / "c.py"))
    monkeypatch.setattr(_shutil, "which", lambda _: str(fake))

    with pytest.raises(DevMapEngineError) as caught:
        find_engine_binary()
    assert "too old" in str(caught.value) or "no devmap binary" in str(caught.value)


def test_a_missing_project_root_fails_closed(tmp_path):
    with pytest.raises(DevMapEngineError):
        build_map(tmp_path / "does-not-exist")


@requires_engine
def test_a_commit_that_changes_no_indexed_file_leaves_the_map_fresh(tmp_path):
    """`generated_head` must describe the tree, not the last persisted generation.

    The Rust `manifest` command stamps `head_sha` from the newest *persisted
    generation* (`latest_generation_head`), which is honest for the store but
    answers a different question than `map_is_stale`, which compares the field
    against the current `git rev-parse HEAD`. Commit without touching an
    indexed file and the incremental build persists no new generation, so the
    stamp keeps pointing at the previous commit and the map reads stale
    *immediately after `dev map` wrote it* — `--if-stale` never short-circuits
    and the watcher rebuilds forever.

    Measured on this repository before the fix: HEAD `e109d16`, stored
    `30daf62`, `map_is_stale` True on a map one second old.

    Both artifacts are asserted: the kernel writes them from one freshness
    identity on purpose, and a map and graph stamped from different commits is
    the drift that single invocation exists to prevent.
    """
    from devcouncil.indexing.repo_mapper import RepoMapper

    root = _git_repo(tmp_path)
    build_map(root)
    subprocess.run(
        [
            "git",
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "a commit that touches no indexed file",
        ],
        cwd=root,
        check=True,
    )
    head = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=root, capture_output=True, text=True, check=True
    ).stdout.strip()

    written = build_map(root)
    payload = json.loads(written.read_text())

    assert payload["generated_head"] == head, "the map must be stamped with the current HEAD"
    assert RepoMapper(project_root=root).map_is_stale(payload) is False, (
        "a map written moments ago must not read stale"
    )

    graph = json.loads((root / ".devcouncil" / "graph" / "code_graph.json").read_text())
    assert graph["generated_head"] == head, "the graph shares the map's freshness identity"
    assert graph["indexed_hash"] == payload["indexed_hash"]
    assert graph["content_fingerprint"] == payload["content_fingerprint"]


@requires_engine
def test_the_kernel_stamps_freshness_so_python_never_rewrites_the_graph(tmp_path, monkeypatch):
    """The digests reach the artifacts without a Python read-modify-write.

    `stamp_freshness` parsed each finished artifact, set three scalars and
    re-serialized the whole thing — 1.68 s of a 2.72 s `dev map` on this
    repository, nearly all of it re-encoding a 26 MB `code_graph.json` the
    kernel had just encoded. Passing the values to the writer removes that
    second serialization.

    Asserting "the fields are populated" alone would not catch a regression:
    the fallback path populates them too, just slowly. So the test pins the
    mechanism — `stamp_freshness` must not run at all when the kernel can be
    told the values — which is the only observable difference between the fast
    path and the slow one.
    """
    root = _git_repo(tmp_path)

    called: list[tuple] = []
    real_stamp = devmap_engine.stamp_freshness

    def spy(*args, **kwargs):
        called.append(args)
        return real_stamp(*args, **kwargs)

    monkeypatch.setattr(devmap_engine, "stamp_freshness", spy)
    map_path = build_map(root)

    assert not called, (
        "the kernel accepts the stamp flags, so the Python read-modify-write "
        "must not run — it re-serializes the entire graph to set three scalars"
    )

    graph_path = root / devmap_engine.DEFAULT_GRAPH_RELPATH
    payloads = {
        "map": json.loads(map_path.read_text()),
        "graph": json.loads(graph_path.read_text()),
    }
    for name, payload in payloads.items():
        for field in ("generated_head", "indexed_hash", "content_fingerprint"):
            assert payload.get(field), f"{name}.{field} was not stamped"

    # One freshness identity for both artifacts, which is the property the
    # single `manifest` invocation exists to guarantee.
    for field in ("generated_head", "indexed_hash", "content_fingerprint"):
        assert payloads["map"][field] == payloads["graph"][field], (
            f"{field} differs between the map and the graph"
        )

    # A stamped digest must stop being advertised as uncomputable. Claiming both
    # would make a consumer skip the check the value could have satisfied.
    unavailable = payloads["graph"]["meta"]["devmap_rust"]["unavailable"]
    assert "indexed_hash" not in unavailable
    assert "content_fingerprint" not in unavailable
    # Reachability is still genuinely not computed; stamping must not imply it.
    assert "unreachable_files" in unavailable
    assert payloads["graph"]["meta"]["liveness_unreachable_unreliable"] is True


@requires_engine
def test_an_older_kernel_without_the_stamp_flags_still_gets_a_stamped_map(tmp_path, monkeypatch):
    """The fallback survives, because an unstamped map reads permanently stale.

    `map_is_stale` skips its fingerprint check only when `generated_head` is
    *also* empty, and the Rust map always carries a head — so an artifact with
    empty digests compares unequal on every call, `--if-stale` never
    short-circuits and the watcher rebuilds forever. A kernel too old for the
    flags must therefore still get the values patched in, slowly, rather than
    going without.
    """
    root = _git_repo(tmp_path)
    monkeypatch.setattr(devmap_engine, "_manifest_accepts_stamp_flags", lambda _binary: False)

    called: list[tuple] = []
    real_stamp = devmap_engine.stamp_freshness

    def spy(*args, **kwargs):
        called.append(args)
        return real_stamp(*args, **kwargs)

    monkeypatch.setattr(devmap_engine, "stamp_freshness", spy)
    map_path = build_map(root)

    assert called, "an older kernel must fall back to the Python stamp"
    payload = json.loads(map_path.read_text())
    for field in ("generated_head", "indexed_hash", "content_fingerprint"):
        assert payload.get(field), f"{field} was not stamped by the fallback"


# --- binary selection: schema first, then the optimized build ----------------
#
# Measured on this repository while the kernel workspace was being built:
# `find_engine_binary` returned `rust-port/target/debug/devmap` because a
# `cargo test` had just written it, and the debug kernel is **5.8x slower** at
# the same work — `manifest` 6.25 / 6.36 / 6.82 s against 0.54 / 1.10 / 1.79 s
# for the release build on the same store with the same argv, interleaved. That
# turned a 0.9 s `dev map` into 8.5 s for anyone who had run the test suite.


def _fake_kernel(path: Path, *, schema: int | None) -> Path:
    """A kernel stand-in that answers both probes `find_engine_binary` makes.

    Real binaries are unusable here: the test needs to control the reported
    schema and the mtime independently, and a real build reports whatever it
    was built from.
    """
    path.parent.mkdir(parents=True, exist_ok=True)
    schema_line = (
        f'  printf \'{{"expected_schema_version": {schema}}}\\n\'\n'
        if schema is not None
        else "  printf '{}\\n'\n"
    )
    path.write_text(
        "#!/bin/sh\n"
        "for arg in \"$@\"; do\n"
        '  [ "$arg" = status ] && is_status=1\n'
        "done\n"
        'if [ -n "$is_status" ]; then\n'
        f"{schema_line}"
        "  exit 0\n"
        "fi\n"
        "echo 'Usage: devmap manifest [OPTIONS] --output <OUTPUT> "
        "--graph-output <GRAPH_OUTPUT>'\n"
    )
    path.chmod(0o755)
    return path


def _only_these_candidates(monkeypatch, tmp_path):
    """Keep the search to the fixture: no package tree, no PATH."""
    import shutil as _shutil

    monkeypatch.setattr(devmap_engine, "__file__", str(tmp_path / "elsewhere" / "b" / "c.py"))
    monkeypatch.setattr(_shutil, "which", lambda _: None)
    devmap_engine._MANIFEST_HELP_CACHE.clear()
    # `getattr` so the red demonstration of these tests is the *selection*
    # failing, not an AttributeError on a cache that did not exist yet.
    getattr(devmap_engine, "_SCHEMA_PROBE_CACHE", {}).clear()
    # The debug-kernel warning fires once per selection; without this a test
    # asserting it would pass or fail on test *order*.
    getattr(devmap_engine, "_DEBUG_KERNEL_WARNED", set()).clear()


def test_an_optimized_build_wins_over_a_newer_debug_build_at_the_same_schema(
    tmp_path, monkeypatch
):
    """Newest-wins picked a 5.8x slower kernel whenever the suite had just run.

    Age was only ever a *proxy* for "was this built after the schema bump".
    The binary answers that question directly, so the proxy is not needed to
    settle it — and when the two builds agree on the schema there is nothing
    left for age to decide except how slow the map is.
    """
    root = tmp_path / "repo"
    release = _fake_kernel(root / "rust-port" / "target" / "release" / "devmap", schema=12)
    debug = _fake_kernel(root / "rust-port" / "target" / "debug" / "devmap", schema=12)
    os.utime(release, (1_000_000, 1_000_000))
    os.utime(debug, (2_000_000, 2_000_000))
    _only_these_candidates(monkeypatch, tmp_path)

    assert find_engine_binary(root) == str(release)


def test_a_newer_schema_still_beats_the_optimized_build(tmp_path, monkeypatch, caplog):
    """Speed never outranks being able to open the store.

    This is the case the age rule was written for, and it must keep working:
    a release build made before a schema bump and a debug build made after it
    both pass the capability probe, because the flags did not change. The
    difference is now read off the binary instead of inferred from its mtime.

    The warning is part of the contract. Selecting a debug kernel costs ~6x on
    every `dev map`, and a cost that large must never be paid silently.
    """
    root = tmp_path / "repo"
    release = _fake_kernel(root / "rust-port" / "target" / "release" / "devmap", schema=12)
    debug = _fake_kernel(root / "rust-port" / "target" / "debug" / "devmap", schema=13)
    os.utime(release, (2_000_000, 2_000_000))
    os.utime(debug, (1_000_000, 1_000_000))
    _only_these_candidates(monkeypatch, tmp_path)

    with caplog.at_level(logging.WARNING, logger="devcouncil.devmap_engine"):
        assert find_engine_binary(root) == str(debug)
    assert any("debug" in record.message for record in caplog.records), caplog.text


def test_with_no_schema_evidence_the_newest_capable_build_still_wins(tmp_path, monkeypatch):
    """A kernel too old to report its schema degrades to the previous rule.

    Preferring `release` on no evidence would resurrect the exact bug the age
    rule was added to fix, so the optimized-build preference applies only where
    the schemas are known *and* equal.
    """
    root = tmp_path / "repo"
    release = _fake_kernel(root / "rust-port" / "target" / "release" / "devmap", schema=None)
    debug = _fake_kernel(root / "rust-port" / "target" / "debug" / "devmap", schema=None)
    os.utime(release, (1_000_000, 1_000_000))
    os.utime(debug, (2_000_000, 2_000_000))
    _only_these_candidates(monkeypatch, tmp_path)

    assert find_engine_binary(root) == str(debug)


def test_the_schema_probe_creates_no_store(tmp_path, monkeypatch):
    """The probe must be a question, not a side effect.

    `status` against a path with no store reports the schema and exits 0. If it
    ever started creating one, every candidate probe would leave a stray store
    behind — and on the real path, would race the build it is selecting for.
    """
    root = tmp_path / "repo"
    _fake_kernel(root / "rust-port" / "target" / "release" / "devmap", schema=12)
    _only_these_candidates(monkeypatch, tmp_path)
    before = sorted(p.name for p in Path(tempfile.gettempdir()).glob("devmap-schema-probe-*"))

    find_engine_binary(root)

    after = sorted(p.name for p in Path(tempfile.gettempdir()).glob("devmap-schema-probe-*"))
    assert after == before
