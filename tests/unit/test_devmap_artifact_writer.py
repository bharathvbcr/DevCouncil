"""Who wrote the consumer artifacts, and how much it costs to find out.

`_artifact` answers "which engine wrote this" by `json.loads`-ing the artifact
itself. For `code_graph.json` that is a 34.5 MB parse — measured at 150.0 ms p50
against 1.3 ms for `repo_map.json` — paid on every `dev map status` and every
doctor run to read one string out of `meta`.

The kernel already writes that string next to the store, in
`<db>.artifacts.json`, at the moment it is known. These tests hold two lines:

  * when the sidecar still describes the file on disk, the engine comes from it
    and the artifact is not parsed at all;
  * when it does not, the answer degrades to *unknown* — never to the verdict
    reserved for an artifact some other writer actually claimed.
"""

from __future__ import annotations

import json
from pathlib import Path

import devcouncil.devmap_health as health
from devcouncil.devmap_engine import (
    DEFAULT_DB_RELPATH,
    DEFAULT_GRAPH_RELPATH,
    DEFAULT_MAP_RELPATH,
)


def _check(result: dict, name: str) -> dict:
    for item in result["checks"]:
        if item["name"] == name:
            return item
    raise AssertionError(f"no {name!r} check in {[c['name'] for c in result['checks']]}")


def _artifacts(root: Path, *, graph_body: dict, map_body: dict) -> None:
    for relpath, body in (
        (DEFAULT_MAP_RELPATH, map_body),
        (DEFAULT_GRAPH_RELPATH, graph_body),
    ):
        path = root / relpath
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(body), encoding="utf-8")


def _sidecar(root: Path, *, engine: str = "devmap-rust", roles=("repo_map", "code_graph")) -> Path:
    """Write a sidecar that truthfully describes the artifacts on disk."""
    relpaths = {"repo_map": DEFAULT_MAP_RELPATH, "code_graph": DEFAULT_GRAPH_RELPATH}
    outputs = []
    for role in roles:
        path = root / relpaths[role]
        stat = path.stat()
        outputs.append(
            {
                "role": role,
                "path": str(path),
                "len": stat.st_size,
                "mtime_ns": stat.st_mtime_ns,
                "ino": stat.st_ino,
                "ctime_ns": stat.st_ctime_ns,
            }
        )
    sidecar = root / (DEFAULT_DB_RELPATH + ".artifacts.json")
    sidecar.parent.mkdir(parents=True, exist_ok=True)
    sidecar.write_text(
        json.dumps(
            {
                "version": 3,
                "writer": "/somewhere/devmap:1:2",
                "map_engine": engine,
                "generated_head": "abc123",
                "inputs": {"generation_id": 7, "content_fingerprint": "c2:abc"},
                "outputs": outputs,
            }
        ),
        encoding="utf-8",
    )
    return sidecar


def test_the_engine_comes_from_the_sidecar_without_parsing_the_graph(tmp_path: Path) -> None:
    """A sidecar that still describes the file answers, and the artifact is not read.

    The whole point: `code_graph.json` is the biggest file the tool writes, and
    the question being asked of it is one short string the kernel already
    recorded elsewhere.
    """
    _artifacts(
        tmp_path,
        map_body={"map_engine": "devmap-rust"},
        graph_body={"meta": {"map_engine": "devmap-rust"}},
    )
    _sidecar(tmp_path)

    graph = tmp_path / DEFAULT_GRAPH_RELPATH
    opened: list[str] = []
    real_read_text = Path.read_text

    def _spy(self: Path, *args, **kwargs):  # type: ignore[no-untyped-def]
        opened.append(str(self))
        return real_read_text(self, *args, **kwargs)

    original = Path.read_text
    Path.read_text = _spy  # type: ignore[method-assign]
    try:
        info = health.artifacts_info(tmp_path)
    finally:
        Path.read_text = original  # type: ignore[method-assign]

    assert info["code_graph"]["map_engine"] == "devmap-rust"
    assert info["code_graph"]["writer_verified"] is True
    assert str(graph) not in opened, (
        "code_graph.json was parsed even though the sidecar answers for it; "
        f"read: {opened}"
    )


def test_a_stale_sidecar_falls_back_to_the_artifact(tmp_path: Path) -> None:
    """A sidecar that no longer describes the file must not answer for it.

    It is evidence about bytes that are gone. Falling back to the artifact is
    what the doctor did before the sidecar existed, so this can only ever cost
    the parse it was meant to save — never a wrong answer.
    """
    _artifacts(
        tmp_path,
        map_body={"map_engine": "devmap-rust"},
        graph_body={"meta": {"map_engine": "devmap-rust"}},
    )
    _sidecar(tmp_path)

    # The graph is rewritten after the sidecar was taken: different length.
    graph = tmp_path / DEFAULT_GRAPH_RELPATH
    graph.write_text(
        json.dumps({"meta": {"map_engine": "some-other-mapper"}, "padding": "x" * 64}),
        encoding="utf-8",
    )

    info = health.artifacts_info(tmp_path)
    assert info["code_graph"]["writer_verified"] is False
    assert info["code_graph"]["map_engine"] == "some-other-mapper", (
        "a stale sidecar must not keep vouching for an engine; the artifact's "
        "own claim is what is left"
    )


def test_an_artifact_that_claims_no_engine_is_unverified_not_foreign(tmp_path: Path) -> None:
    """The three states must stay three.

    `map_engine != "devmap-rust"` folded two different findings into one verdict:
    an artifact some other tool *claimed* to write, and an artifact that makes no
    claim at all. Only the first is evidence of a foreign writer. Reporting the
    second as `foreign_writer` is a check that could not run reporting the result
    of a check that ran and failed — and it fails the doctor critically, so a map
    with an unlabelled artifact is called broken on no evidence.
    """
    _artifacts(
        tmp_path,
        map_body={"map_engine": "devmap-rust"},
        graph_body={"meta": {}},  # no map_engine anywhere, and no sidecar
    )

    result = health.run_doctor(tmp_path)
    item = _check(result, "code_graph")

    assert item["code"] != "foreign_writer", (
        "an artifact that names no engine is not evidence that a foreign writer "
        "wrote it; it is evidence of nothing"
    )
    assert item["code"] == "artifact_writer_unverified"
    assert item["ok"] is None, "unknown is not False"
    assert item["critical"] is False


def test_an_artifact_claiming_another_engine_is_still_foreign(tmp_path: Path) -> None:
    """The finding that does have evidence behind it must survive."""
    _artifacts(
        tmp_path,
        map_body={"map_engine": "devmap-rust"},
        graph_body={"meta": {"map_engine": "some-other-mapper"}},
    )

    result = health.run_doctor(tmp_path)
    item = _check(result, "code_graph")

    assert item["ok"] is False
    assert item["code"] == "foreign_writer"
    assert "some-other-mapper" in item["detail"]
