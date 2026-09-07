"""A request the kernel refuses is an error, not a reason to answer elsewhere.

`_devmap_query_payload` returns `None` on `DevMapClientError` so the caller can
fall back to the Python graph engine. That is right for "the kernel is not
here" and wrong for "the kernel will not take this request": a query over the
byte cap, a non-UTF-8 argument, a depth out of range. Measured on this
repository, a 200 KB `dev map query` was refused by the client validator in
microseconds and then answered by the fallback in 3.9 s — a whole-graph load of
117k payloads to scan for a string no symbol can contain — while a non-UTF-8
argument escaped the validator as a `UnicodeEncodeError` traceback.
"""

from __future__ import annotations

from pathlib import Path

import pytest
from typer.testing import CliRunner


def test_non_utf8_query_is_refused_by_name_not_by_traceback() -> None:
    from devcouncil.devmap_client import DevMapClient, DevMapRequestRefused

    # What `sys.argv` hands a CLI for the bytes ff fe: surrogate escapes.
    with pytest.raises(DevMapRequestRefused, match="UTF-8"):
        DevMapClient._validate_query("\udcff\udcfe")


def test_oversized_query_is_a_refusal_subclass() -> None:
    from devcouncil.devmap_client import (
        MAX_QUERY_BYTES,
        DevMapClientError,
        DevMapRequestRefused,
    )

    with pytest.raises(DevMapRequestRefused, match="exceeds") as info:
        from devcouncil.devmap_client import DevMapClient

        DevMapClient._validate_query("x" * (MAX_QUERY_BYTES + 1))
    # Existing `except DevMapClientError` sites keep catching it.
    assert isinstance(info.value, DevMapClientError)


class _RefusingClient:
    def search(self, query, limit=2000, semantic=False):
        from devcouncil.devmap_client import DevMapRequestRefused

        raise DevMapRequestRefused("devmap query exceeds 4096 UTF-8 bytes")


class _AbsentKernelClient:
    def search(self, query, limit=2000, semantic=False):
        from devcouncil.devmap_client import DevMapClientError

        raise DevMapClientError("devmap search failed: connection refused")


def test_payload_surfaces_a_refusal_instead_of_falling_back(monkeypatch, tmp_path: Path) -> None:
    from devcouncil.cli.commands import graph_cmd
    import devcouncil.devmap_client as devmap_client

    monkeypatch.setattr(devmap_client, "try_connect", lambda root: _RefusingClient())
    result = graph_cmd._devmap_query_payload(tmp_path, "query", name_or_path="x" * 5000)

    assert result is not None, "a refused request must not be re-run on the Python engine"
    assert "exceeds" in str(result.get("error"))
    assert result.get("source") == "devmap"


def test_payload_still_falls_back_when_the_kernel_is_absent(monkeypatch, tmp_path: Path) -> None:
    from devcouncil.cli.commands import graph_cmd
    import devcouncil.devmap_client as devmap_client

    monkeypatch.setattr(devmap_client, "try_connect", lambda root: _AbsentKernelClient())
    assert graph_cmd._devmap_query_payload(tmp_path, "query", name_or_path="widget") is None


def test_cli_query_refusal_exits_one_without_loading_the_python_graph(monkeypatch, tmp_path: Path) -> None:
    from devcouncil.cli.main import app
    import devcouncil.devmap_client as devmap_client
    import devcouncil.indexing.graph.query as graph_query

    monkeypatch.setattr(devmap_client, "try_connect", lambda root: _RefusingClient())

    def _never(*args, **kwargs):
        raise AssertionError("the Python graph engine must not be consulted for a refused request")

    # `query.py` reads the kernel's `code_graph.json` through `read_code_graph`
    # now, not the retired store through `load_code_graph`; this guards whatever
    # whole-graph read the module holds, so it follows the name.
    monkeypatch.setattr(graph_query, "read_code_graph", _never)
    monkeypatch.chdir(tmp_path)
    result = CliRunner().invoke(app, ["map", "query", "x" * 5000, "--project-root", str(tmp_path)])

    assert result.exit_code == 1, result.output
    assert "exceeds" in result.output


def test_cli_search_refusal_is_an_error_not_an_empty_result(monkeypatch, tmp_path: Path) -> None:
    from devcouncil.cli.main import app
    import devcouncil.devmap_client as devmap_client

    monkeypatch.setattr(devmap_client, "try_connect", lambda root: _RefusingClient())
    monkeypatch.chdir(tmp_path)
    result = CliRunner().invoke(app, ["map", "search", "x" * 5000, "--project-root", str(tmp_path)])

    # Before: exit 0 with no lines — indistinguishable from "searched, found nothing".
    assert result.exit_code == 1, result.output
    assert "exceeds" in result.output
