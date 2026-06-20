"""ITEM A: the LSP/symbol-index surface is honest (detection-only) and the dead
symbol_index module is gone."""

from __future__ import annotations

import importlib

from devcouncil.indexing.lsp import LspInspector


def test_lsp_summary_is_labelled_detection_only(tmp_path):
    (tmp_path / "app.py").write_text("def hello():\n    return 1\n", encoding="utf-8")

    summary = LspInspector(tmp_path).summary(["app.py"])

    # Honest mode marker + human-readable disclaimer.
    assert summary["mode"] == "detection-only"
    assert "does not run an LSP client" in summary["note"]
    # Detection still works.
    assert summary["languages"] == ["python"]
    assert summary["detected_servers"][0]["language"] == "python"


def test_lsp_initialize_payloads_are_marked_not_sent(tmp_path):
    (tmp_path / "app.py").write_text("def hello():\n    return 1\n", encoding="utf-8")

    summary = LspInspector(tmp_path).summary(["app.py"])

    # The initialize payloads carry an explicit "never sent" disclaimer so they
    # are not mistaken for a live capability.
    init = summary["initialize_requests"]
    assert "NEVER sent" in init["_note"]
    # The starter payload itself is still present and well-formed (back-compat).
    assert init["python"]["method"] == "initialize"


def test_starter_payload_method_name_is_honest(tmp_path):
    inspector = LspInspector(tmp_path)
    # The method is named to signal it is a starter, not a live request sender.
    assert hasattr(inspector, "starter_initialize_payload")
    payload = inspector.starter_initialize_payload("python")
    assert payload["method"] == "initialize"


def test_symbol_index_module_is_deleted():
    # The 0-byte dead module must no longer be importable.
    import pytest

    with pytest.raises(ModuleNotFoundError):
        importlib.import_module("devcouncil.indexing.symbol_index")


def test_indexing_package_imports_cleanly():
    # No dangling references to the deleted module break the package.
    import devcouncil.indexing.repo_mapper as repo_mapper
    import devcouncil.indexing.lsp as lsp

    importlib.reload(lsp)
    importlib.reload(repo_mapper)
    assert repo_mapper.RepoMapper is not None


def test_repo_mapper_role_files_have_no_symbol_index_reference():
    from devcouncil.indexing.repo_mapper import RepoMapper

    role_specs = RepoMapper._SUBSYSTEM_ROLE_FILES["src/devcouncil/indexing"]
    all_tokens = [token for _, tokens in role_specs for token in tokens]
    assert not any("symbol_index" in token for token in all_tokens)
    # Curated entry-point list, too.
    summary, entry_points = RepoMapper._SUBSYSTEM_INDEX["src/devcouncil/indexing"]
    assert not any("symbol_index" in p for p in entry_points)


def test_graph_index_kept_because_referenced():
    # GraphIndex is consumed by integrations/gitnexus.py, so it must remain importable.
    from devcouncil.indexing.graph_index import GraphIndex

    assert GraphIndex is not None
