"""Shared pytest fixtures.

Several modules memoize expensive state in module-level caches keyed by resolved
project root (the parsed config, local secrets, the SQLAlchemy ``Database`` handle,
the MCP server's router/adapter caches, and the skill registry). Every test uses a
unique ``tmp_path`` so these caches don't collide in practice, but the autouse
fixture below clears them after each test as a defensive guarantee — so a test that
deletes/recreates state under a path a later test happens to reuse can never receive
a stale cached instance.
"""

from __future__ import annotations

import os
from pathlib import Path

import pytest


@pytest.fixture(scope="session", autouse=True)
def _isolate_devcouncil_logs(tmp_path_factory):
    """Keep test logging out of the real repo's .devcouncil/logs/devcouncil.log.

    Tests exercise deliberate failures (missing executors, invalid transitions,
    fake GitHub 403s); without this, that noise lands in the developer's actual
    log file and drowns out real errors. DEVCOUNCIL_LOG_DIR is honored by
    devcouncil.telemetry.logging_setup for both initial configuration and
    set_log_dir() re-pointing.
    """
    log_dir = tmp_path_factory.mktemp("devcouncil-test-logs")
    previous = os.environ.get("DEVCOUNCIL_LOG_DIR")
    os.environ["DEVCOUNCIL_LOG_DIR"] = str(log_dir)
    # Never let a test spawn a `devmap serve` daemon. Measured on 2026-09-02:
    # one run of the map tests left 288 daemons behind — one per temporary
    # repository, each holding a deleted store for its 30-minute idle bound.
    # A test that needs the daemon starts it explicitly and reaps it.
    previous_spawn = os.environ.get("DEVMAP_AUTOSPAWN")
    os.environ["DEVMAP_AUTOSPAWN"] = "0"
    yield
    if previous is None:
        os.environ.pop("DEVCOUNCIL_LOG_DIR", None)
    else:
        os.environ["DEVCOUNCIL_LOG_DIR"] = previous
    if previous_spawn is None:
        os.environ.pop("DEVMAP_AUTOSPAWN", None)
    else:
        os.environ["DEVMAP_AUTOSPAWN"] = previous_spawn


def _reset_all_caches() -> None:
    # Each reset is best-effort and independent: importing one module must not
    # prevent the others from being cleared.
    try:
        from devcouncil.storage.db import reset_db_cache

        reset_db_cache()
    except Exception:
        pass
    try:
        from devcouncil.integrations.mcp.server import _reset_caches

        _reset_caches()
    except Exception:
        pass
    try:
        from devcouncil.skills.registry import clear_skill_caches

        clear_skill_caches()
    except Exception:
        pass
    try:
        from devcouncil.app import config

        config._CONFIG_CACHE.clear()
        config._SECRETS_CACHE.clear()
        # Drop any cached gcloud token so a monkeypatched token in one test can't leak
        # into the next via the 50-minute TTL cache.
        config._gcloud_token_cache = None
    except Exception:
        pass


@pytest.fixture(autouse=True)
def _clear_module_caches():
    yield
    _reset_all_caches()


#: The developer's own index, which no unit test may touch. `devmap.sqlite` is
#: the Rust kernel's store, and every file here is one the kernel writes.
#:
#: `index.sqlite` (the Python query cache) and `writer.lock` (its build lease)
#: were watched here too, until the Python store was deleted: nothing creates
#: either any more, so watching them could only ever report "unchanged" — the
#: shape of a check that cannot run reporting as one that ran and passed.
_REPO_STATE_FILES = (
    ".devcouncil/codeintel/devmap.sqlite",
    ".devcouncil/repo_map.json",
    ".devcouncil/graph/code_graph.json",
)


def _repo_state_fingerprint() -> dict[str, tuple[int, int]]:
    """`(size, mtime_ns)` of the real repository's map state, absent as `(-1, -1)`.

    Absence is a state like any other: a test that *creates* one of these has
    escaped its `tmp_path` just as surely as one that rewrites it.
    """
    root = Path(__file__).resolve().parent.parent
    fingerprint: dict[str, tuple[int, int]] = {}
    for relative in _REPO_STATE_FILES:
        try:
            stat = (root / relative).stat()
            fingerprint[relative] = (stat.st_size, stat.st_mtime_ns)
        except OSError:
            fingerprint[relative] = (-1, -1)
    return fingerprint


@pytest.fixture(autouse=True)
def _repo_map_state_is_not_collateral(request: pytest.FixtureRequest):
    """Fail the test that built the developer's own map, by name.

    A test that forgets `tmp_path` (or hands a `PromptBuilder`, a
    `RepoMapper` or the codeintel service a default `project_root`) resolves to
    the repository the suite is running in and indexes *it*. That is not a
    hypothetical: `test_prompt_builder_wraps_paths_and_commands_as_markdown_code`
    took no `tmp_path`, so `PromptBuilder._graph_impact_lines` called
    `load_code_graph(<repo root>)` and imported this repository's 34 MB
    `code_graph.json` into a 94 MB `.devcouncil/codeintel/index.sqlite` — 66 s
    of the file's runtime, inside an `except Exception` that made it silent.
    (That function and that store are both deleted now; the escape it describes
    is not, which is why this fixture stays.)
    Its sibling `test_prompt_builder_injects_applicable_skills` did the same
    thing and was only found because this check ran per test: the
    session-scoped version of it said *that* something escaped and left the
    *which* to a bisect over 4,400 tests on a machine loaded enough to
    reproduce it.

    Two costs, both paid by the developer rather than by the test: the suite
    rewrites the map the developer is working against, and the test's result
    then depends on that machine's map — the same class as picking up a stale
    globally-installed kernel.

    Deliberately a stat comparison and nothing more, five `stat` calls per
    test. This is a tripwire, not a sandbox; a test that legitimately needs a
    store builds one under `tmp_path`.

    **Its blind spot, measured rather than guessed.** The create-if-absent
    artifact this warned about was `index.sqlite`: `load_code_graph` wrote it
    only when it was missing, so once a contaminated run had created it every
    later run merely read it and this comparison saw nothing move. Observed
    directly: one full run created it at 15:47:31 and tripped; the identical run
    immediately afterwards passed clean while the 94 MB file sat there the whole
    time. That file is no longer written by anything and is no longer watched,
    so the specific blind spot is closed — but the *shape* of it is not. Every
    file above is rewritten unconditionally by the kernel, so a green run is
    real evidence for those three; the caveat returns the moment a
    create-if-absent artifact is added to the watched set.
    """
    before = _repo_state_fingerprint()
    yield
    after = _repo_state_fingerprint()
    moved = sorted(name for name in before if before[name] != after[name])
    if moved:
        pytest.fail(
            f"{request.node.nodeid} modified the repository's own map state, "
            "which means it escaped its tmp_path and indexed this checkout: "
            + ", ".join(moved)
            + " — give it `tmp_path` and `monkeypatch.chdir(tmp_path)`, or pass "
            "it an explicit project root",
            pytrace=False,
        )
