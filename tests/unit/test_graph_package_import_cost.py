"""What importing one module out of `devcouncil.indexing.graph` drags in.

Importing *any* submodule of a package runs that package's `__init__` first.
`devcouncil/indexing/graph/__init__.py` re-exported its public surface with
eager `from .build import …` / `from .export import …` / `from .intel import …`
/ `from .query import …` lines, so the cheapest possible import out of the
package paid for all of it.

Two module-scope importers on the `dev map` hot path did exactly that:

- `devcouncil/indexing/map_artifacts.py` imported `graph.schema.CodeGraph`,
  used in one place and only as a type annotation;
- `devcouncil/indexing/repo_mapper.py` imported `graph.cache.PARSE_CACHE_VERSION`,
  a single integer out of a 77-line module.

Measured with `python -X importtime` on `dev map` over this repository:
`devcouncil.indexing.graph` cost **136.6 ms** cumulative, of which
`graph.build` was 125.7 ms and the pydantic tree beneath it 49.6 ms — to obtain
one annotation and one integer.

Asserting on `sys.modules` rather than elapsed time, for the reason
`test_app_package_import_cost` and `test_hook_import_cost` both give: a timing
assertion is flaky on a loaded machine and says nothing about *why* it got
slow. The heavy modules being absent is the same fact, checked exactly.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

#: Submodules of `devcouncil.indexing.graph` that no cheap import may pull in.
#: `schema` and `cache` are deliberately absent — they are the light leaves the
#: hot-path importers actually want.
HEAVY_SUBMODULES = (
    "devcouncil.indexing.graph.build",
    "devcouncil.indexing.graph.export",
    "devcouncil.indexing.graph.intel",
    "devcouncil.indexing.graph.query",
)


def _modules_after(statement: str) -> set[str]:
    """Fully-qualified module names loaded by *statement* in a fresh interpreter.

    A subprocess, not `importlib.reload`: the point is what a *cold* import
    pulls in, and this test process has already imported most of the tree.

    The probe is pinned to the `devcouncil` package *this* process imported. In
    a git worktree the interpreter's editable install points at the primary
    checkout, so a bare `python -c "import devcouncil…"` measures a different
    tree than the one under test — and reports its state as this one's.
    """
    import devcouncil

    source_root = Path(devcouncil.__file__).resolve().parent.parent
    env = dict(os.environ)
    env["PYTHONPATH"] = os.pathsep.join(
        [str(source_root), *([env["PYTHONPATH"]] if env.get("PYTHONPATH") else [])]
    )
    probe = subprocess.run(
        [
            sys.executable,
            "-c",
            f"{statement}\n"
            "import devcouncil, json, sys\n"
            "print(json.dumps({'root': devcouncil.__file__, "
            "'modules': sorted(sys.modules)}))",
        ],
        capture_output=True,
        text=True,
        check=True,
        env=env,
    )
    payload = json.loads(probe.stdout)
    assert Path(payload["root"]).resolve() == Path(devcouncil.__file__).resolve(), (
        "the probe imported a different checkout than the test process: "
        f"{payload['root']} vs {devcouncil.__file__}"
    )
    return set(payload["modules"])


def test_importing_the_parse_cache_does_not_build_the_graph():
    """`repo_mapper` wants one integer out of `graph.cache`."""
    loaded = _modules_after("import devcouncil.indexing.graph.cache")
    for heavy in HEAVY_SUBMODULES:
        assert heavy not in loaded, (
            f"importing graph.cache must not load {heavy}; the package "
            "__init__ is re-exporting it eagerly again"
        )


def test_importing_the_graph_schema_does_not_build_the_graph():
    """`map_artifacts` wants one name out of `graph.schema`."""
    loaded = _modules_after("import devcouncil.indexing.graph.schema")
    for heavy in HEAVY_SUBMODULES:
        assert heavy not in loaded, (
            f"importing graph.schema must not load {heavy}; the package "
            "__init__ is re-exporting it eagerly again"
        )


def test_importing_map_artifacts_does_not_build_the_graph_or_load_the_orm():
    """The `dev map` hot path: this module is imported on every invocation."""
    loaded = _modules_after("import devcouncil.indexing.map_artifacts")
    for heavy in (*HEAVY_SUBMODULES, "sqlalchemy", "sqlmodel"):
        assert heavy not in loaded, (
            f"importing map_artifacts must not load {heavy}; it is on the "
            "`dev map` and PostToolUse hook paths and uses none of it"
        )


def test_every_re_exported_name_still_resolves():
    """Lazy must not mean absent.

    `devcouncil.indexing.graph` is imported for its facade by production
    callers and by tests; every name in `__all__` has to keep resolving as a
    module attribute, or the deferral is a breakage rather than a saving.
    """
    from devcouncil.indexing import graph

    missing = [name for name in graph.__all__ if getattr(graph, name, None) is None]
    assert not missing, f"names in __all__ that no longer resolve: {missing}"


def test_an_unknown_attribute_is_still_an_attribute_error():
    from devcouncil.indexing import graph

    try:
        graph.NoSuchName
    except AttributeError as error:
        assert "NoSuchName" in str(error)
    else:  # pragma: no cover - the assertion above is the contract
        raise AssertionError("an unknown attribute must raise AttributeError")
